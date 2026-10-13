-- Migration: 038_mcp_server.sql
-- External AI-agent access (Model Context Protocol, Streamable HTTP).
--
-- Umbrel's `umbrelMcp` module is the reference: one master kill-switch, one
-- global permission set shared by every token (per-token scoping is only
-- revocation), opaque `prefix_id_secret` bearer tokens whose SHA-256 hash is
-- stored. HomeVault mirrors that shape in two tables:
--
-- - `mcp_settings` (singleton row id = 1): `enabled` master switch plus the
--   global `permissions` JSON (`crate::mcp::McpPermissions`).
-- - `mcp_tokens`: one row per issued token. Only the SHA-256 of the secret
--   half is stored; verification is a constant-time compare, so a database
--   read alone never yields a usable token.

CREATE TABLE IF NOT EXISTS mcp_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    enabled INTEGER NOT NULL DEFAULT 0,
    permissions TEXT NOT NULL DEFAULT '{}',
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT OR IGNORE INTO mcp_settings (id, enabled, permissions)
VALUES (1, 0, '{}');

CREATE TABLE IF NOT EXISTS mcp_tokens (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    secret_hash TEXT NOT NULL,
    prefix TEXT NOT NULL,
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_used_at TEXT
);
