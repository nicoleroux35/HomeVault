-- Migration: 041_resource_shares.sql
-- Fine-grained per-member access to apps, folders and SMB/NFS shares.
--
-- HomeVault already had coarse roles (user/admin) and POSIX-ACL-backed share
-- permissions, but no way for the owner to say "this member may use *this*
-- installed app / open *this* folder / mount *this* share". This table is that
-- map: one row grants `member` the `permission` level on `resource`, which is
-- owned by `owner`.
--
-- `kind` is one of:
--   'app'    — a Docker/compose app id (e.g. 'jellyfin')
--   'folder' — an absolute path under a home or a shared root
--   'share'  — an SMB/NFS/FTP/WebDAV share name
--
-- `permission` is one of 'read' | 'write' | 'full'. The server filters every
-- listing (app catalog, explorer roots, share list) and every WebSocket event
-- through this table — a member never sees a resource they have no row for.
--
-- Owner rows are implicit: the account that owns the instance (the first
-- admin, HomeVault's equivalent of the appliance "owner") sees everything and
-- needs no row here. Admins likewise bypass the filter.

CREATE TABLE IF NOT EXISTS resource_shares (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    owner TEXT NOT NULL REFERENCES users(username) ON DELETE CASCADE,
    member TEXT NOT NULL REFERENCES users(username) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    resource TEXT NOT NULL,
    permission TEXT NOT NULL DEFAULT 'read',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    CHECK (kind IN ('app', 'folder', 'share')),
    CHECK (permission IN ('read', 'write', 'full')),
    UNIQUE (member, kind, resource)
);

CREATE INDEX IF NOT EXISTS idx_resource_shares_member ON resource_shares(member, kind);
CREATE INDEX IF NOT EXISTS idx_resource_shares_owner ON resource_shares(owner, kind);
CREATE INDEX IF NOT EXISTS idx_resource_shares_resource ON resource_shares(kind, resource);
