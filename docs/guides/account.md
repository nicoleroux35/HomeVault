# My Account (self-service)

**Purpose:** The logged-in user's own profile, password, and 2FA management,
opened from the desktop topbar user menu.

## Backend

- Profile/password/2FA all reuse the auth endpoints: `GET /auth/me`,
  `POST /auth/2fa/setup` (`action: disable` / `backup_codes`), and password change
  via the user-update-password path used by the Users page.
- `POST /auth/2fa/setup` with `action: disable` requires the current password.

## Frontend

- `frontend/src/routes/Account.svelte`.
- `frontend/src/lib/services/api.ts`: `disableMy2FA`, `getMy2FABackupCodes`.
- Opened from `frontend/src/routes/Desktop.svelte` (lazy `Account` import,
  `openAccount`, topbar user dropdown "My Account" item).
- The `isSelf` branch in `Users.svelte::handlePassword` shares the same
  session-invalidation behavior.

## Invariants

- **Self password change invalidates every session for that account server-side,
  including the one making the request.** The page logs out and redirects to
  `/login` after a self password change (same fix required in the Users page's
  `isSelf` branch — keep the two in sync).
- A full-page loading flicker after every save/2FA action was removed.
- Disabling 2FA requires a non-empty password (validation error otherwise).
- **Editable since 2026-09-12** — `CLAUDE.md`'s frozen-area registry is gone, so
  `Account.svelte`, its two api.ts methods, the `Account` import in
  `Desktop.svelte` and the `account.*` i18n keys are open to edits. The
  invariants above stay binding: a change keeps the Rust integration +
  Playwright E2E suites green.

## Related

- [Users](./users.md)
- [Authentication](./auth.md)
