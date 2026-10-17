# Security (sessions & login history)

**Purpose:** Let a user review/revoke their own sessions and review login history
(admin-only per-user history).

## Content Security Policy (2026-10-17)

`script-src` no longer allows `'unsafe-inline'`. The policy lists a **per-response
nonce** instead (`middleware::csp_nonce`):

- `security_headers_middleware` — the outermost layer — generates 16 random
  bytes (hex) per request and inserts them as `CspNonce` in the request
  extensions;
- `handlers::spa::serve_spa` replaces the `__HV_CSP_NONCE__` placeholder in
  `index.html` with that exact value, so the one inline boot script (the
  service-worker registration) carries `nonce="…"` matching the header;
- the two must agree **within one response** — a test asserts it.

Why it matters: with `'unsafe-inline'` and the session token in `localStorage`,
any injected script could read the token and exfiltrate a full session. A nonce
is unguessable, so injected markup cannot run.

`style-src` **keeps** `'unsafe-inline'`: Svelte emits scoped `<style>` blocks
and components set `style="…"` attributes, neither of which a nonce can cover.
Accepted debt — CSS cannot read the session token.

Adding an inline script later means adding `nonce="__HV_CSP_NONCE__"` to it in
`frontend/index.html`; anything without it will simply not run.

## Brute-force protection (2026-10-17)

There is deliberately **no fail2ban module**. An earlier
`src/middleware/fail2ban.rs` defined an in-memory IP ban list that no route or
handler ever referenced — dead code that read like an active protection. It was
removed; brute-force defence lives in two places instead:

- **Per account** — 5 failed logins inside 5 minutes lock the account, and the
  lock records an expiry (`users.locked_until`), so it lifts itself. See
  [auth.md](./auth.md) → *Login lockout and throttling*.
- **Per client IP** — 30 attempts/minute and 120/hour on `/auth/login` **and**
  `/auth/2fa/verify`, keyed by the TCP peer address.

Do not reintroduce an unwired "protection" module: either wire it and test it,
or leave it out.

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
