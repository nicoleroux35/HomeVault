-- Migration: 008_permissions_schema.sql
-- Groupes HomeVault, quotas et permissions de partage

-- Groupes HomeVault (mapping DB <-> /etc/group)
CREATE TABLE IF NOT EXISTS hv_groups (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT UNIQUE NOT NULL,
    full_name TEXT UNIQUE NOT NULL,
    gid INTEGER UNIQUE,
    description TEXT,
    created_at TEXT NOT NULL,
    is_system INTEGER NOT NULL DEFAULT 0
);

-- Quotas par groupe (en Ko)
CREATE TABLE IF NOT EXISTS hv_quotas (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    group_id INTEGER NOT NULL,
    filesystem TEXT NOT NULL,
    soft_limit INTEGER NOT NULL DEFAULT 0,
    hard_limit INTEGER NOT NULL DEFAULT 0,
    inode_soft INTEGER NOT NULL DEFAULT 0,
    inode_hard INTEGER NOT NULL DEFAULT 0,
    current_usage INTEGER NOT NULL DEFAULT 0,
    current_inodes INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (group_id) REFERENCES hv_groups(id) ON DELETE CASCADE,
    UNIQUE(group_id, filesystem)
);

-- Permissions de partage (DB <-> ACL filesystem)
CREATE TABLE IF NOT EXISTS share_permissions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    share_name TEXT NOT NULL,
    share_type TEXT NOT NULL,
    grantee_type TEXT NOT NULL,
    grantee_name TEXT NOT NULL,
    permission TEXT NOT NULL,
    recursive INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(share_name, share_type, grantee_type, grantee_name)
);

-- Index pour performances
CREATE INDEX IF NOT EXISTS idx_quota_group ON hv_quotas(group_id);
CREATE INDEX IF NOT EXISTS idx_share_perm_lookup ON share_permissions(share_name, share_type);
CREATE INDEX IF NOT EXISTS idx_share_perm_grantee ON share_permissions(grantee_type, grantee_name);
CREATE INDEX IF NOT EXISTS idx_hv_groups_gid ON hv_groups(gid);
