-- Web Push subscriptions (phase D2, docs/guides/web-push.md).
--
-- One row per browser/device a user explicitly subscribed. The three values
-- are the browser's PushSubscription: its endpoint (a capability URL), its
-- P-256 public key and its 16-byte authentication secret. They are not
-- returned by the API; the screen shows the endpoint host and a label only.
--
-- Deleting the account removes its devices. The sender uses the same table to
-- record delivery outcomes, and removes a subscription the push service
-- reports as gone (404/410) or one that has failed too many times.
CREATE TABLE IF NOT EXISTS push_subscriptions (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL,
    endpoint TEXT NOT NULL,
    p256dh TEXT NOT NULL,
    auth TEXT NOT NULL,
    device_label TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    last_success_at TEXT,
    last_failure_at TEXT,
    failure_count INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (username) REFERENCES users(username) ON DELETE CASCADE,
    UNIQUE (username, endpoint)
);

CREATE INDEX IF NOT EXISTS idx_push_subscriptions_username
    ON push_subscriptions(username);
