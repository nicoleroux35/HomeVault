-- Migration: 051_resource_group_shares.sql
-- Group-level resource grants (2026-10-20).
--
-- `resource_shares` (migration 041) grants one *member* access to one app,
-- folder or share. Managing that row per person does not scale on a family
-- appliance: "everyone in `family` may use Jellyfin" had to be re-typed for
-- each new account, and a member added to the group later inherited nothing.
--
-- This table is the group-level equivalent. `group_name` is a HomeVault group
-- (the display name the Groups app shows, i.e. `hv_<name>` without the
-- prefix). There is deliberately no foreign key: groups live in the OS
-- (`/etc/group`), not in SQLite — `hv_groups` is only a metadata mirror.
--
-- Resolution is the union of both tables, strongest permission winning
-- (`full` ⊃ `write` ⊃ `read`), computed at read time from the member's *live*
-- OS group membership (`GroupService::get_user_groups`). Nothing is copied
-- into `resource_shares`, so removing someone from a group revokes the
-- inherited access immediately — there is no denormalised cache to go stale.
--
-- A direct member row and a group row can both match; the effective level is
-- the stronger of the two. Revoking the group row therefore never demotes a
-- member who also holds a direct grant.
CREATE TABLE IF NOT EXISTS resource_group_shares (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    owner TEXT NOT NULL REFERENCES users(username) ON DELETE CASCADE,
    group_name TEXT NOT NULL,
    kind TEXT NOT NULL,
    resource TEXT NOT NULL,
    permission TEXT NOT NULL DEFAULT 'read',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    CHECK (kind IN ('app', 'folder', 'share')),
    CHECK (permission IN ('read', 'write', 'full')),
    UNIQUE (group_name, kind, resource)
);

CREATE INDEX IF NOT EXISTS idx_resource_group_shares_group
    ON resource_group_shares(group_name, kind);
CREATE INDEX IF NOT EXISTS idx_resource_group_shares_resource
    ON resource_group_shares(kind, resource);
