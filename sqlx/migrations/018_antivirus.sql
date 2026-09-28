-- Antivirus (ClamAV) scanning: on-demand + scheduled scans, quarantine.
-- Real-time/on-write scanning (clamd daemon) is intentionally out of scope —
-- HomeVault targets low-power boards/mini-PCs and a resident scan daemon
-- would work against the resource ceilings in installer/homevault.service.

-- Singleton settings row (id is always 1).
CREATE TABLE IF NOT EXISTS av_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    quarantine_dir TEXT NOT NULL DEFAULT '/var/lib/homevault/quarantine',
    scheduled_scan_enabled INTEGER NOT NULL DEFAULT 0,
    scheduled_scan_cron TEXT NOT NULL DEFAULT '0 3 * * 0',
    scheduled_scan_path TEXT,
    scan_archives INTEGER NOT NULL DEFAULT 1,
    last_scan_at TEXT,
    updated_at TEXT NOT NULL
);

-- History of scan runs (manual or scheduled).
CREATE TABLE IF NOT EXISTS av_scans (
    id TEXT PRIMARY KEY,
    scan_path TEXT NOT NULL,
    trigger TEXT NOT NULL,
    status TEXT NOT NULL,
    files_scanned INTEGER NOT NULL DEFAULT 0,
    threats_found INTEGER NOT NULL DEFAULT 0,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    error_message TEXT
);

-- Threats found during a scan, quarantined (moved) by clamscan --move.
CREATE TABLE IF NOT EXISTS av_threats (
    id TEXT PRIMARY KEY,
    scan_id TEXT NOT NULL REFERENCES av_scans(id) ON DELETE CASCADE,
    file_path TEXT NOT NULL,
    signature_name TEXT NOT NULL,
    action TEXT NOT NULL,
    quarantine_path TEXT,
    detected_at TEXT NOT NULL,
    resolved_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_av_threats_scan_id ON av_threats(scan_id);
