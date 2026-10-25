# Security (sessions & login history)

**Purpose:** Let a user review/revoke their own sessions and review login history
(admin-only global history; per-user history is available to an admin or to the
account owner).

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
| `GET /security/sessions` | auth | current user's sessions, each identified by an **opaque id** (`SessionInfo.id`) — the response never carries a live session token |
| `POST /security/sessions/revoke` | auth | revoke sessions by opaque id (`{ ids: [] }`); ids resolve **only** against the caller's own sessions, so a foreign id/token is a 404 and deletes nothing |
| `POST /security/sessions/revoke-all` | auth | revoke every session for the caller except the current one (caller-scoped by construction) |
| `GET /security/login-history` | admin | global login history — each row is **truthful**: the real `success` flag (failures stay, they are security-relevant) and the HTTP `User-Agent`; per-user failure counters only count `success = 0`, `clear_attempts` never purges successful logins |
| `GET /security/login-history/{username}` | admin or self | per-user history (same truthful success/user-agent per row); a non-admin may only read their own |
| `GET /security/active-sessions-count` | admin | distinct users with a currently valid session, system-wide (not the caller's own sessions) — powers the desktop "Active Sessions" monitor widget |

### Session identifiers (2026-10-22)

`session_public_id(token)` (`src/handlers/security.rs`) is a 12-character
SHA-256 fingerprint of the session token. It is what `GET /security/sessions`
returns and what `POST /security/sessions/revoke` accepts:

- **Never the token.** The old response mapped `token: s.token.clone()`, which
  put a replayable bearer token into the DOM, devtools, any client-side log and
  any XSS on the account page. The admin view already masked its tokens
  (`handlers/users.rs::mask_session_token`); the self-service view now returns
  no token at all.
- **Ownership is enforced at resolution, not at deletion.** The revoke handler
  loads `find_all_by_username(caller)` and keeps only the sessions whose public
  id matches the request. A token/id belonging to another account is not in the
  candidate set, so it cannot be revoked even if the attacker knows it
  (CWE-639). A request that resolves nothing answers **404** ("No matching
  session to revoke for your account"), an empty `ids` array answers **400**.
- **The current session stays protected.** Its own id never resolves (it is
  filtered before deletion); "log out everything else" is the dedicated
  `revoke-all` endpoint.

### One auth primitive, one status contract (2026-10-24, ROADMAP X.7 / X.8)

Every handler in `security.rs` used to parse `x-session-token` itself and rebuild
the same three branches (header present → resolve → is-admin). Six copies of the
same decision is how the answer drifts, and it had:

- `security.rs`, `shares.rs`, `permissions.rs`, `snmp.rs` and `settings.rs`
  answered **`400 Bad Request`** to a signed-in **non-admin** where every other
  module answered `403`;
- a **missing** `x-session-token` answered **`400`** across the API (the header
  was "just another absent parameter") while `terminal_pty.rs`, `system.rs` and
  a few others answered `401`.

A client that branches on the status therefore read "you are not signed in" as
"your request is malformed", and the same refusal looked like two different
problems on two screens. Both are fixed at the root:

- `handlers::admin_guard` is now **the only module that turns the header into an
  identity decision**. The eight modules that carried a private
  `require_admin` (`syncthing`, `shares`, `vpn`, `ups`, `permissions`, `snmp`,
  `user_homes`, `downloader`), the `Explorer`/`Settings`/`Chat`/`Avatars` inline
  blocks and `handlers::auth::require_admin` (a duplicate of the shared one) all
  delegate to it. Modules that answer a `Json<ApiResponse>` envelope keep their
  envelope and flatten the guard's *status* to a message via
  `admin_guard::denial_message`.
- The status is stated once, in `admin_guard.rs` and in
  [API.md](../API.md) → *Auth status contract*: `401` for an absent/empty header
  or an unknown/expired token, `403` for a valid non-admin on an `admin` route,
  and `400` **only** for a request the server could not understand.
- Source guards in `admin_guard.rs` fail the build if a module defines its own
  `require_admin`/`require_auth` again, or if one of the eight delegated modules
  starts parsing the header itself. Behaviour guard:
  `auth_guard_integration.rs` (missing → `401`, non-admin → `403`, malformed →
  `400`, on the same routes).

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

- Revoking a session invalidates that `X-Session-Token` server-side immediately.
  `revoke-all` keeps the caller's **current** session alive — it signs out the
  *other* devices only, so the current client does not need to log in again.
- **No live session token is ever returned by the self-service API** — only the
  opaque id from `session_public_id`. Guard:
  `account_integration.rs::test_my_sessions_never_returns_a_live_session_token`.
- **Revocation is caller-scoped.** `POST /security/sessions/revoke` resolves the
  submitted ids against the caller's own sessions only; a foreign token/id is a
  404 and leaves the other account signed in. Guard:
  `account_integration.rs::test_revoke_sessions_is_scoped_to_the_caller`.
- Login history: the global view is admin-only; per-user history is admin **or**
  the account owner (`handle_get_user_login_history`).
- **Every handler in this module guards through `admin_guard`** — no local copy
  of the token check, no local status choice. Source guard:
  `admin_guard.rs::security_module_never_parses_the_header_itself`.
- **An expired session is `401` on all three self-service endpoints** and writes
  nothing. Guard:
  `account_integration.rs::test_security_endpoints_reject_an_expired_session`.
- `getActiveSessionsCount` counts **distinct usernames** with a
  non-expired session (`SessionRepository::count_distinct_active_users`),
  not raw session rows — one user open in several tabs/devices still
  counts once. Don't reuse `SessionRepository::count()` for this: it
  counts every row including already-expired ones not yet swept by
  `delete_expired`, and doesn't dedupe by user.

## Related

- [Authentication](./auth.md), [Settings](./settings.md)
