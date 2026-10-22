# My Account (self-service)

**Purpose:** The logged-in user's own profile, password, 2FA management, active
sessions and recent sign-in history, opened from the desktop topbar user menu.

## Backend

- Profile/password/2FA all reuse the auth endpoints: `GET /auth/me`,
  `POST /auth/2fa/setup` (`action: disable` / `backup_codes`), and password change
  via the user-update-password path used by the Users page.
- `POST /auth/2fa/setup` with `action: disable` requires the current password.

### Active sessions

- `GET /security/sessions` — the caller's own sessions. Each row carries an
  **opaque `id`** (a 12-char SHA-256 fingerprint of the token), `ip_address`,
  `created_at`, `expires_at` and `is_current`. The live session token is
  **never** in the payload.
- `POST /security/sessions/revoke` — `{ ids: [] }`; resolves each id only
  against the caller's own sessions (a foreign id/token is a 404), refuses the
  current session, and returns `{ revoked: n }`.
- `POST /security/sessions/revoke-all` — logs out every session except the
  current one (`{ revoked: n }`).

### Recent sign-ins

- `GET /security/login-history/{username}` — the caller's own history (an admin
  may request any username). Returns `attempts[]` with `username`, `ip_address`,
  `success`, `timestamp` and `user_agent`.

## Frontend

- `frontend/src/routes/Account.svelte` — two `SectionCard`s: "Active sessions"
  (per-row revoke button, "This device" badge, "Log out all others" with a
  confirm dialog, refresh) and "Recent logins".
- `frontend/src/lib/services/api.ts`: `disableMy2FA`, `getMy2FABackupCodes`,
  `getMySessions`, `revokeSessions(ids)`, `revokeAllSessions`,
  `getUserLoginHistory`.
- Opened from `frontend/src/routes/Desktop.svelte` (lazy `Account` import,
  `openAccount`, topbar user dropdown "My Account" item).
- The `isSelf` branch in `Users.svelte::handlePassword` shares the same
  session-invalidation behavior.

## Invariants

- **Self password change invalidates every session for that account server-side,
  including the one making the request.** The page logs out and redirects to
  `/login` after a self password change (same fix required in the Users page's
  `isSelf` branch — keep the two in sync).
- **The session cards work on opaque ids, never on live tokens** — the list
  payload and the DOM must not contain `x-session-token` values. See
  [Security](./security.md) → *Session identifiers*.
- A full-page loading flicker after every save/2FA action was removed.
- Disabling 2FA requires a non-empty password (validation error otherwise).
- **Editable since 2026-09-12** — `CLAUDE.md`'s frozen-area registry is gone, so
  `Account.svelte`, its api.ts methods, the `Account` import in
  `Desktop.svelte` and the `account.*` i18n keys are open to edits. The
  invariants above stay binding: a change keeps the Rust integration +
  Playwright E2E suites green.

## Related

- [Users](./users.md)
- [Authentication](./auth.md)
- [Security](./security.md)
