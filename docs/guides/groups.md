# Groups Management

**Purpose:** Admin management of user groups and membership. Shared `GroupService` with the Users page.

## Backend

Handlers in `src/handlers/groups.rs`; service in `src/groups.rs`
(`GroupService::list_groups`, `is_admin_group`).

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /groups` | admin | list groups |
| `POST /groups` | admin | create (name ≤32, lowercase/numbers/hyphens) |
| `GET /groups/{name}` | admin | detail + members |
| `DELETE /groups/{name}` | admin | delete |
| `POST /groups/{name}/members` | admin | add member `{ username }` |
| `DELETE /groups/{name}/members/{username}` | admin | remove member |
| `GET /groups/users` | admin | available users |
| `GET /users/{username}/groups` | admin | groups of a user |

## Frontend

- `frontend/src/routes/Groups.svelte`.
- `frontend/src/lib/services/api.ts`: `getGroups`, `getGroup`, `createGroup`,
  `deleteGroup`, `getAvailableUsers`, `addMemberToGroup`, `removeMemberFromGroup`.
- `src/groups.rs` also backs the Users page's group assignment, so edits to one
  must be checked against the other (a regression once hid the real admin group
  from this page).

## Invariants

- The `homevault_admin` group is hidden and protected here too
  (`GroupService::is_admin_group`). It cannot be deleted/renamed through this API.
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
