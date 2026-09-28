-- Firewall management (ufw). Rules themselves are never mirrored in the
-- database (read live from `ufw status numbered`, like src/vpn.rs reads
-- `wg show` live) to avoid drift between the DB and the real system state.
-- Only settings and a genuine audit trail are persisted here.

-- Singleton settings row (id is always 1). `enabled` is a best-effort cache
-- for quick reads; live `ufw status` is always the source of truth.
CREATE TABLE IF NOT EXISTS firewall_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    enabled INTEGER NOT NULL DEFAULT 0,
    ssh_port INTEGER NOT NULL DEFAULT 22,
    app_port INTEGER NOT NULL DEFAULT 8080,
    confirmed_at TEXT,
    updated_at TEXT NOT NULL
);

-- Audit trail of firewall changes made through HomeVault.
CREATE TABLE IF NOT EXISTS firewall_change_log (
    id TEXT PRIMARY KEY,
    action TEXT NOT NULL,
    rule_description TEXT,
    performed_by TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_firewall_change_log_created ON firewall_change_log(created_at);
