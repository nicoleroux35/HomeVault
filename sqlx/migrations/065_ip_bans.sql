-- Login auto-block (QUICK-WIN QW-05).
--
-- Two tables, because they answer two different questions:
--
--   ip_bans            "which addresses are blocked right now, and until when"
--   auto_block_settings "how does this box decide to block one"
--
-- The bans live in the database — not in the in-memory limiter — precisely so
-- that a restart does not hand an attacker a clean slate. `firewall_rule` records
-- whether a ufw rule was also installed, so a pardon (or an expiry sweep) can
-- remove exactly what was added and nothing else.
CREATE TABLE IF NOT EXISTS ip_bans (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ip TEXT NOT NULL,
    reason TEXT NOT NULL,
    -- How many failures tripped it; kept for the Security card's explanation.
    failures INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    -- 1 when `ufw deny from <ip>` was installed for this ban.
    firewall_rule INTEGER NOT NULL DEFAULT 0
);

-- One live ban per address: re-banning an already-banned IP extends it rather
-- than stacking rows the UI would have to de-duplicate.
CREATE UNIQUE INDEX IF NOT EXISTS idx_ip_bans_ip ON ip_bans(ip);
-- The sweep runs on every enforcement check; the expiry index keeps it O(log n).
CREATE INDEX IF NOT EXISTS idx_ip_bans_expires ON ip_bans(expires_at);

CREATE TABLE IF NOT EXISTS auto_block_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    enabled INTEGER NOT NULL DEFAULT 0,
    -- Failures inside `window_seconds` that trip a ban.
    threshold INTEGER NOT NULL DEFAULT 5,
    window_seconds INTEGER NOT NULL DEFAULT 300,
    ban_seconds INTEGER NOT NULL DEFAULT 1800,
    -- Also install/remove a ufw rule. Off by default: locking an address out of
    -- every service is a bigger hammer than a refused login, and a home user
    -- must opt in to it.
    apply_firewall INTEGER NOT NULL DEFAULT 0,
    -- Request-rate cap on the public listener ("DoS" row).
    dos_enabled INTEGER NOT NULL DEFAULT 1,
    dos_requests_per_minute INTEGER NOT NULL DEFAULT 600,
    updated_at TEXT
);
