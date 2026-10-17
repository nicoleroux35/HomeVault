# Authentication, 2FA, Recovery Code & Sessions

**Purpose:** Documents login, two-factor auth, first-run setup status, the
login lockout and throttling rules, the offline recovery-code account-recovery
flow, and session management. Covers the Login/Auth area.

## Backend

Handlers in `src/handlers/auth.rs`; service in `src/auth/service.rs`; types in
`src/storage/models.rs`. Session token helpers in `src/handlers/admin_guard.rs`.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `POST /auth/login` | public | returns session token; `requires_2fa: true` if 2FA enabled. Also sets a `hv_session` cookie (proxy-only). `429` when the per-IP throttle or the per-account lockout trips. |
| `POST /auth/2fa/verify` | public | completes login with TOTP code; same IP throttle as `/auth/login` |
| `POST /auth/2fa/setup` | auth | `action`: `generate`/`verify`/`disable`/`status`/`backup_codes` |
| `POST /auth/logout` | auth | invalidates the session, clears the cookie |
| `GET /auth/me` | auth | current user + roles |
| `POST /auth/recovery-code` | auth | generate the one-time recovery code (400 if one exists unless `?regenerate=true`) |
| `POST /auth/verify-recovery-code` | public | `{ username, code }` → opens the new-password step |
| `POST /auth/reset-password-recovery` | public | `{ username, code, new_password }` — the code is **not** consumed |
| `GET /auth/setup-status` | public | `is_first_run` (derived from the admin row in `users`), `is_setup_valid`, `hardware_changed`, `needs_recovery`, `server_machine_id`, `sudo_available`, `system_language`. Never cached by the SPA. |
| `GET /auth/check-username` | public (local network only) | availability check |
| `POST /auth/setup` | public (local network only) | create first admin (Setup Wizard); accepts optional `totp_secret` + `totp_code`, returns extra `totp_enabled` + `backup_codes`. Refused once an admin exists (DB check, not a marker file). |
| `POST /auth/setup/verify-totp` | public (local network only, first-run only) | stateless pre-creation TOTP check: `{ secret, code }` → `{ valid }` (403 once an admin exists) |
| `POST /auth/setup/rebaseline` | admin | re-baselines the recorded hardware fingerprint after a confirmed hardware change (recovery flow); writes `setup_state` + logs an event. |

> The three unauthenticated bootstrap routes (`/auth/setup`,
> `/auth/setup/verify-totp`, `/auth/check-username`) are additionally
> restricted to loopback / private / link-local peers by
> `middleware::setup_guard`, **and** to same-origin browser contexts (a
> cross-site `Origin` / `Sec-Fetch-Site: cross-site` is refused with 403 — the
> CORS layer is permissive, so without it any website could claim the first
> admin from the operator's own browser), **and** rate-limited to 20 requests
> per minute per peer (429).
> `HOMEVAULT_SETUP_ALLOW_REMOTE=1` lifts only the local-network check; the
> same-origin and rate-limit checks always apply. `/auth/setup-status` is exempt
> so the login screen can read it from anywhere. See `setup-wizard.md`.

## Password policy (single source of truth)

`SetupService::validate_password` (`src/setup.rs`) is the only policy:

- **12 characters minimum** (was 8) and 128 maximum — the admin credential owns
  the whole appliance;
- at least one uppercase, one lowercase and one digit;
- not on the compromised/obvious list (`BANNED_PASSWORDS`: `password…`,
  `qwerty…`, `homevault…`, `motdepasse…`, `passwort…`, `changeme…`, …), matched
  case-insensitively and also with trailing digits stripped, so `Password1234`
  and `Homevault2026` are caught;
- no character repeated more than 4 times in a row (`Aaaaaaaa…`).

`AuthService::validate_password_strength` (password change, admin-created users,
recovery-code reset) delegates to the same rules, so a password accepted by the
wizard can never be refused later — and vice versa. The wizard's live checklist
lists all six rules and is what enables its "Next" button.

> The email/token "forgot password" flow (`/auth/forgot-password`,
> `/auth/reset-password`, `password_reset_tokens`, the reset-token
> `AuthService` methods) was **deleted** 2026-09-03 (migration `029`). The
> `email` module itself stays — it is still used for notifications.

## Login lockout and throttling (2026-10-17)

Two independent axes, because one alone is not enough:

### Per account — a *temporary* lock

`MAX_LOGIN_ATTEMPTS` (5) failures inside `LOCKOUT_DURATION_SECONDS` (300) lock
 the account. The lock now records **when it expires**, in the new
`users.locked_until` column (migration `047`):

| `locked` | `locked_until` | Meaning |
|---|---|---|
| 0 | – | normal |
| 1 | set | automatic lock — `AuthService::release_expired_temporary_lock` lifts it as soon as the timestamp has passed, and a successful login clears it too |
| 1 | `NULL` | deliberate lock (admin action, `PUT /users/{username}/lock`) — only an admin (or a recovery-code reset) lifts it |

Before this, five wrong passwords set `locked = 1` and nothing ever cleared it:
on a single-admin appliance that bricked the only account — there is no support
desk, and the message said "temporarily locked". The migration repairs accounts
already bricked by the counter (a lock with ≥ 5 recorded failed attempts and no
admin action) by giving them an expiry; admin-locked accounts stay permanent.

`set_user_locked` clears `locked_until` in both directions, so an admin lock can
never be undone by a stale expiry, and a recovery-code reset unlocks outright.

### Per client IP — a throttle

The per-account counter cannot see **password spraying** (one password tried
across many usernames). `AuthService::check_ip_rate_limit` runs on every
authentication attempt, before the account lookup, with two windows:
`LOGIN_IP_MAX_PER_MINUTE` (30) and `LOGIN_IP_MAX_PER_HOUR` (120), keyed by the
client IP. It covers `/auth/login` **and** `/auth/2fa/verify`, since both go
through `AuthService::authenticate`. Exceeding either window returns **429**
(`AuthError::TooManyAttempts`); the handler now honours that status instead of
collapsing every non-401 error into a 400.

### The client IP is the TCP peer

`handlers::auth::client_ip(headers, peer)` uses the connection's peer address,
never a header alone. `X-Forwarded-For` / `X-Real-IP` are client-settable, so
trusting them (as the code did) let anyone forge the address in the login
history and — once the throttle is keyed by IP — sidestep it by rotating a
header value. The headers are honoured only when the peer is loopback (a reverse
proxy on this host) or listed in `HOMEVAULT_TRUSTED_PROXIES` (comma-separated
IPs). The peer address arrives through `middleware::PeerAddr`, an optional
extractor, so in-process callers and tests simply get `None`.

### The session cookie

`hv_session` is `HttpOnly` + `SameSite=Lax`, and carries `Secure` **only when
the request arrived over TLS**. Unconditional `Secure` would be silently
dropped by the browser on a plain-HTTP LAN deployment and break the app proxy;
`over_tls` comes from the `ArrivedOverTls` extension. See
[tls.md](./tls.md) → *The session cookie follows the connection*.

### What the user sees

The locked-account message points at the way back in — wait five minutes, or use
**Forgot password?** with the recovery code (which unlocks immediately) —
instead of "contact support". The inline error under the fields is a
`role="alert"` / `aria-live="assertive"` region, the credential fields carry
`aria-invalid` + `aria-describedby`, and closing the error modal returns focus to
the form rather than dropping it on `<body>`.

## Account recovery (offline numeric code)

- One 32-digit code (8 groups of 4, ≈106 bits) is generated by
  `AuthService::generate_recovery_code`, shown **once** on a mandatory screen at
  the end of the Setup Wizard, and never again. Only its Argon2 hash is stored
  (`password_recovery_codes`, `src/storage/password_recovery_repo.rs`,
  migration `028`). The screen is **blocking**: if generation fails, the wizard
  offers a retry and refuses to continue (there is no email reset, so the code
  is the only offline way back in).
- The Login "forgot password?" panel takes username + code (dashes/whitespace
  ignored via `normalize_recovery_code`). `verify_recovery_code` is rate-limited
  5/15 min under a `recovery:<username>` key — **separate** from the login
  lockout, so a bad code never locks the account.
- `reset_password_with_recovery_code` runs the same strength/history checks and
  session invalidation as the old token path, and **does not consume the code**
  (stays valid for a future reset).
- Only accounts created through the Setup Wizard *after* 2026-09-03 have a code.
  Self-service / admin-side regeneration is still deferred.

## Two-factor authentication (TOTP) enrolment

- Since 2026-09-22 the Setup Wizard enrols TOTP **at the password step**, before
  the account exists: the admin flips a toggle, the secret is generated
  **client-side** (`crypto.getRandomValues`, 20 bytes, RFC 4648 base32) and the
  QR code (otpauth URI, SHA1 / 6 digits / 30 s, issuer `HomeVault`) is rendered
  with the `qrcode` npm package — compatible with Google Authenticator,
  Microsoft Authenticator, Aegis, Ente Auth, 2FAS and other standard TOTP apps.
- The entered 6-digit code is validated with the stateless
  `POST /auth/setup/verify-totp` endpoint (`AuthService::verify_totp_code_with_secret`,
  window **−1..=1** time step — one past period tolerated so codes entered at
  the edge of a period are not rejected a moment later). The authoritative
  check runs **again** inside `POST /auth/setup` before any row is created: an
  invalid code is a clean rejection with nothing persisted.
- On success `handle_setup` calls `enable_2fa` (best-effort — a DB failure
  leaves the account usable without 2FA, surfaced via `totp_enabled: false`)
  and returns the one-time backup codes in the setup response. The one-time
  backup codes are shown on a short summary screen after the recovery code.
- Existing self-service enrolment in "My Account" is unchanged
  (`POST /auth/2fa/setup` with `action: generate`/`verify`).

## Frontend

- `frontend/src/lib/stores/appGate.ts` — the application route guard
  (`refresh`/`enterApp`/`signOut`). It reads `/auth/setup-status` and the local
  session and decides between the Setup Wizard, the hardware-change recovery
  screen, login, the desktop, or a connection-error screen; `App.svelte`
  renders from `$appGate.route`. `Login.svelte` no longer reads the setup status
  or renders the wizard inline.
- `frontend/src/routes/Login.svelte` — login form + 2FA step + recovery-code
  panel, plus the `recoveryMode` hardware-confirmation banner and the
  `connectionError` screen (both driven by `App.svelte`). The recovery-code
  `<input>` is one-way `value=` + `oninput` (`formatRecoveryCode` /
  `handleRecoveryCodeInput`) — never `bind:value`.
- `frontend/src/routes/SetupWizard.svelte` — first-run admin creation with TOTP
  enrolment at the password step (toggle → QR → verified code), then the
  mandatory "save your recovery code" screen (`startRecoveryCode`), then a short
  backup-codes summary when 2FA was enrolled. Both the recovery code and the
  one-time backup codes are non-selectable (no copy/download).
- `frontend/src/lib/stores/auth.ts` — session state, token persistence.
- `frontend/src/lib/services/api.ts` — `login`, `loginWith2FA`, `logout`,
  `getCurrentUser`, `getSetupStatus`, `setupAdmin`, `checkUsername`,
  `verifySetupTotp`, `setup2FASecret`, `verify2FASecret`, `disableMy2FA`,
  `getMy2FABackupCodes`, `verifyRecoveryCode`, `resetPasswordRecovery`.

## Invariants

- TOTP secrets are base32; `encode_base32`/`decode_base32` must round-trip (a
  past bug corrupted secrets containing W/X/Y/Z).
- A 2FA-enabled account cannot finish login at `/auth/login`; the code must go to
  `/auth/2fa/verify`.
- The recovery code is never stored or logged in plaintext; the Setup display
  stays non-copyable; `verify_recovery_code`'s rate-limit key stays namespaced
  `recovery:`; a successful reset must **not** delete the code.
- `is_admin` checks the DB first (matching `create_session`), falling back to a
  group check only for DB-less accounts.
- The JSON API reads only the `x-session-token` header — the `hv_session` cookie
  is for the per-app reverse proxy only (`src/app_proxy`), no new CSRF surface.
- No screen is frozen since 2026-09-12 — describe behaviour here, and when you
  do edit the Login/Setup-Wizard/auth source, keep this guide and the suites
  (`tests/auth_integration.rs`, `tests/e2e/login`) in the same change.

## Related

- [Setup Wizard](./setup-wizard.md)
- [My Account](./account.md)
- [Security (sessions)](./security.md)
- [API Contract](../API.md)
