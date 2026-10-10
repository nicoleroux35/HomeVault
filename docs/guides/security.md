# Security (sessions & login history)

**Purpose:** Let a user review/revoke their own sessions and review login history
(admin-only per-user history).

## Backend

Handlers in `src/handlers/security.rs`.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /security/sessions` | auth | current user's sessions |
| `POST /security/sessions/revoke` | auth | revoke specific tokens (`{ tokens: [] }`) |
| `POST /security/sessions/revoke-all` | auth | revoke every session for the user |
| `GET /security/login-history` | admin | global login history — each row is **truthful**: the real `success` flag (failures stay, they are security-relevant) and the HTTP `User-Agent`; per-user failure counters only count `success = 0`, `clear_attempts` never purges successful logins |
| `GET /security/login-history/{username}` | admin | per-user history (same truthful success/user-agent per row) |
| `GET /security/active-sessions-count` | admin | distinct users with a currently valid session, system-wide (not the caller's own sessions) — powers the desktop "Active Sessions" monitor widget |

## Frontend

- Security UI: the Settings security-category cards
  (`frontend/src/lib/components/settings/TlsCard.svelte`,
  `FirewallCard.svelte`, `AntivirusCard.svelte`, `AntiRansomwareCard.svelte`)
  plus the login-history view in `frontend/src/routes/Account.svelte`.
- `frontend/src/lib/services/api.ts`: `getMySessions`, `revokeSessions`,
  `revokeAllSessions`, `getLoginHistory`, `getUserLoginHistory`,
  `getActiveSessionsCount`.
- The "Active Sessions" desktop monitor widget (`frontend/src/routes/
  Desktop.svelte`, `.hv-sessions-monitor`) is a separate consumer of
  `getActiveSessionsCount`, owned by the Widgets app (`docs/guides/widgets.md`).

## Invariants

- Revoking sessions invalidates the corresponding `X-Session-Token`s server-side;
  after `revoke-all` the current client must log in again.
- Login history is admin-only (per-user history requires admin).
- `getActiveSessionsCount` counts **distinct usernames** with a
  non-expired session (`SessionRepository::count_distinct_active_users`),
  not raw session rows — one user open in several tabs/devices still
  counts once. Don't reuse `SessionRepository::count()` for this: it
  counts every row including already-expired ones not yet swept by
  `delete_expired`, and doesn't dedupe by user.

## Related

- [Authentication](./auth.md), [Settings](./settings.md)
