-- Migration: 083_account_managers.sql
-- Delegated local-account management (DSM's "Local user and group
-- management" role).
--
-- A row here lets a non-admin manage **non-administrator** local accounts and
-- non-protected groups without becoming an application admin. The delegation
-- is deliberately stored in SQLite rather than an OS group: HomeVault users
-- are DB-only until they need a Linux account, and a delegated manager must
-- not gain a shell just to be delegated.
--
-- The guard is `AuthService::can_manage_accounts`; every handler that accepts
-- it still refuses to touch an administrator, the caller's own account, or the
-- protected `homevault_admin` / `homevault_hostadmin` groups.
CREATE TABLE IF NOT EXISTS account_managers (
    username TEXT PRIMARY KEY REFERENCES users(username) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
