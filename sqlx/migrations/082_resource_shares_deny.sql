-- Migration: 082_resource_shares_deny.sql
-- Allow an explicit `deny` level in the fine-grained resource grants.
--
-- DSM's shared-folder rule is "No access > Read/Write > Read only": a deny
-- wins over every allow, whether it comes from the member's own row or from a
-- group they belong to. `resource_shares` / `resource_group_shares` only
-- allowed `read`/`write`/`full`, so the only way to express "this member must
-- not use Jellyfin even though `family` may" was to remove them from the
-- group. This migration widens the CHECK constraints; the merge rule itself
-- lives in `ResourceShareRepository::merge_effective`.
--
-- SQLite cannot alter a CHECK constraint in place, so both tables are rebuilt
-- with the same columns, defaults, foreign keys and indexes.

CREATE TABLE resource_shares_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    owner TEXT NOT NULL REFERENCES users(username) ON DELETE CASCADE,
    member TEXT NOT NULL REFERENCES users(username) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    resource TEXT NOT NULL,
    permission TEXT NOT NULL DEFAULT 'read',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    CHECK (kind IN ('app', 'folder', 'share')),
    CHECK (permission IN ('deny', 'read', 'write', 'full')),
    UNIQUE (member, kind, resource)
);

INSERT INTO resource_shares_new (id, owner, member, kind, resource, permission, created_at)
    SELECT id, owner, member, kind, resource, permission, created_at FROM resource_shares;

DROP TABLE resource_shares;
ALTER TABLE resource_shares_new RENAME TO resource_shares;

CREATE INDEX IF NOT EXISTS idx_resource_shares_member ON resource_shares(member, kind);
CREATE INDEX IF NOT EXISTS idx_resource_shares_owner ON resource_shares(owner, kind);
CREATE INDEX IF NOT EXISTS idx_resource_shares_resource ON resource_shares(kind, resource);

CREATE TABLE resource_group_shares_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    owner TEXT NOT NULL REFERENCES users(username) ON DELETE CASCADE,
    group_name TEXT NOT NULL,
    kind TEXT NOT NULL,
    resource TEXT NOT NULL,
    permission TEXT NOT NULL DEFAULT 'read',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    CHECK (kind IN ('app', 'folder', 'share')),
    CHECK (permission IN ('deny', 'read', 'write', 'full')),
    UNIQUE (group_name, kind, resource)
);

INSERT INTO resource_group_shares_new (id, owner, group_name, kind, resource, permission, created_at)
    SELECT id, owner, group_name, kind, resource, permission, created_at FROM resource_group_shares;

DROP TABLE resource_group_shares;
ALTER TABLE resource_group_shares_new RENAME TO resource_group_shares;

CREATE INDEX IF NOT EXISTS idx_resource_group_shares_group
    ON resource_group_shares(group_name, kind);
CREATE INDEX IF NOT EXISTS idx_resource_group_shares_resource
    ON resource_group_shares(kind, resource);
