-- P1 — transfer quota usage history and threshold alerts.
--
-- One row per subject per sampling tick (hourly), retained for 90 days. The
-- alert is created when usage crosses 90% within the same counting period; the
-- pure `should_alert` rule lives in src/quota_history.rs.
CREATE TABLE IF NOT EXISTS transfer_quota_history (
    id TEXT PRIMARY KEY,
    observed_at TEXT NOT NULL,
    subject_type TEXT NOT NULL CHECK (subject_type IN ('user', 'group')),
    subject_name TEXT NOT NULL,
    period TEXT NOT NULL,
    upload_used_bytes INTEGER NOT NULL,
    upload_limit_bytes INTEGER NOT NULL,
    download_used_bytes INTEGER NOT NULL,
    download_limit_bytes INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_transfer_quota_history_subject
    ON transfer_quota_history (subject_type, subject_name, observed_at DESC);
CREATE INDEX IF NOT EXISTS idx_transfer_quota_history_observed
    ON transfer_quota_history (observed_at);
