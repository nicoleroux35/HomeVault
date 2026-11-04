-- User-defined host → upstream reverse-proxy rules (phase C2,
-- docs/guides/reverse-proxy.md). HomeVault serves them from one host-routed
-- listener; this table is only the configuration.
--
-- Rules are ordered by host and path prefix. `path` is '' (whole host) or a
-- '/'-prefixed segment prefix; the longest matching prefix wins. The unique
-- constraint is case-insensitive on the host because DNS names are.
--
-- No secrets live here: an upstream is a LAN address, the auth gate reuses the
-- app-proxy session store, and tickets are in memory only.
CREATE TABLE IF NOT EXISTS reverse_proxy_rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Normalised: lower-case, no port, IPv6 bracketed.
    host TEXT NOT NULL COLLATE NOCASE,
    -- '' or '/prefix' (no trailing slash).
    path TEXT NOT NULL DEFAULT '',
    -- Canonical host:port, IPv6 bracketed.
    upstream TEXT NOT NULL,
    -- The upstream speaks HTTPS (the listener itself stays plain HTTP).
    tls INTEGER NOT NULL DEFAULT 0,
    -- Session gate before forwarding; on by default.
    auth_required INTEGER NOT NULL DEFAULT 1,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (host, path)
);

CREATE INDEX IF NOT EXISTS idx_reverse_proxy_rules_enabled
    ON reverse_proxy_rules(enabled, host);
