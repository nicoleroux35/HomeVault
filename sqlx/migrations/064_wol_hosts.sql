-- Wake-on-LAN hosts (QUICK-WIN QW-09).
--
-- A saved host is a label plus the NIC address that should be woken. The
-- address is stored in one canonical form (uppercase, colon-separated) so a
-- lookup by value and a duplicate check are both trivial; `broadcast` is an
-- optional IPv4/IPv6 literal to send to instead of the interface's directed
-- broadcast (a relay, another VLAN, or a test sink).
CREATE TABLE IF NOT EXISTS wol_hosts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    label TEXT NOT NULL,
    mac TEXT NOT NULL,
    broadcast TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Two rows with the same label would make the scheduled-task picker ambiguous.
CREATE UNIQUE INDEX IF NOT EXISTS idx_wol_hosts_label ON wol_hosts(label COLLATE NOCASE);
