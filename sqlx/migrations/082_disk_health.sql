-- Disk health history and findings (tasklist P0-8).
--
-- Samples are one row per watched SMART attribute per run, so a trend can be
-- computed without re-reading smartctl. Findings are deduplicated by a stable
-- fingerprint (serial + attribute + condition): a disk that stays bad raises
-- one row whose `last_seen` moves, not one row per sample.

CREATE TABLE IF NOT EXISTS disk_health_samples (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    serial TEXT NOT NULL,
    model TEXT NOT NULL DEFAULT '',
    attr_id INTEGER NOT NULL,
    raw INTEGER NOT NULL,
    normalized INTEGER,
    sampled_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_disk_health_samples_serial_attr
    ON disk_health_samples (serial, attr_id, sampled_at);

CREATE TABLE IF NOT EXISTS disk_health_findings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    fingerprint TEXT NOT NULL UNIQUE,
    serial TEXT NOT NULL,
    model TEXT NOT NULL DEFAULT '',
    kind TEXT NOT NULL,
    severity TEXT NOT NULL,
    message TEXT NOT NULL,
    first_seen TEXT NOT NULL,
    last_seen TEXT NOT NULL,
    resolved_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_disk_health_findings_active
    ON disk_health_findings (resolved_at, severity, last_seen);
