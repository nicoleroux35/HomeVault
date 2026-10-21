# Groups Management

**Purpose:** Admin management of user groups and membership. Shared `GroupService` with the Users page.

## Backend

Handlers in `src/handlers/groups.rs`; service in `src/groups.rs`
(`GroupService::list_groups`, `is_admin_group`).

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /groups` | admin | list groups |
| `POST /groups` | admin | create (name ≤32, lowercase/numbers/hyphens; optional `description`) |
| `GET /groups/{name}` | admin | detail + members |
| `PUT /groups/{name}` | admin | rename and/or set the description — `{ name?, description? }` |
| `DELETE /groups/{name}` | admin | delete |
| `POST /groups/{name}/members` | admin | add member `{ username }` |
| `DELETE /groups/{name}/members/{username}` | admin | remove member |
| `GET /groups/users` | admin | available users |
| `GET /users/{username}/groups` | admin | groups of a user |

### Rename and description (2026-10-20)

`PUT /groups/{name}` (`handle_group_update`) accepts `{ name?, description? }`;
both are optional, so the same call renames, describes, or does both.

A rename is not just `groupmod -n`. Everything that keys on the group's
**display name** must follow, or the group silently loses access it was
granted. The handler migrates, after the OS rename succeeded:

| Reference | What moves |
|-----------|------------|
| `hv_groups` mirror | `name` + `full_name` (`homevault_<name>`), `gid` preserved |
| `resource_group_shares` | `group_name` (group-level resource grants, migration 051) |
| `transfer_quotas` | `subject_name` where `subject_type = 'group'` |
| `share_permissions` | `grantee_name` where `grantee_type = 'group'` |

Ordering matters: the OS rename runs **first**, so a refusal (name taken, sudo
missing) leaves every reference untouched. The dependent updates are
best-effort and logged — the OS rename already happened, so a failure there is
an inconsistency to surface, not something to roll back.

The admin group (`homevault_admin`) is refused: it carries the passwordless
sudo grant, and renaming it would leave the sudoers rule dangling.

The description lives in the `hv_groups` mirror only (the OS group database
has no such column), so `GET /groups` fills it in from that table; a group
created straight on the host simply has none. `PUT` creates the mirror row if
it is missing rather than silently dropping the text. `description: ""` clears
it; omitting the field leaves it alone.

`GroupService::rename_group` is OS-only and knows nothing about those tables —
never call it without the handler's migration step.

## Frontend

- `frontend/src/routes/Groups.svelte`.
- `frontend/src/lib/services/api.ts`: `getGroups`, `getGroup`, `createGroup`,
  `updateGroup` (rename/description), `deleteGroup`, `getAvailableUsers`,
  `addMemberToGroup`, `removeMemberFromGroup`.
- The detail pane's "Identity" card (`groups-meta-card`) holds the rename input
  (`groups-rename-input`), the description input (`groups-description-input`)
  and one Save button. It is read-only for the protected admin group
  (`groups-meta-readonly`). Saving a rename re-hydrates the pane from the
  reloaded list, so the next save targets the new name.
- `src/groups.rs` also backs the Users page's group assignment, so edits to one
  must be checked against the other (a regression once hid the real admin group
  from this page).

## Invariants

- The `homevault_admin` group is hidden and protected here too
  (`GroupService::is_admin_group`). It cannot be deleted/renamed through this API.
- A rename must migrate **all four** display-name references listed above in the
  same request; a new table keyed by group name joins that list.
- Member-count cache: the page invalidates `/groups` cache after add/remove/delete
  so counts update immediately (a past bug showed stale counts for ~10 s).
- Error toasts must read `res.message` (a past bug read `res.error`, hiding real
  backend messages).
- **Editable since 2026-09-12** — `CLAUDE.md`'s frozen-area registry is gone, so
  `Groups.svelte`, its api.ts methods, `src/handlers/groups.rs` and
  `src/groups.rs` are open to edits. The invariants above stay binding: a change
  keeps the Rust integration + Playwright E2E suites green.

## Related

- [Users](./users.md)
- [Authentication](./auth.md)
