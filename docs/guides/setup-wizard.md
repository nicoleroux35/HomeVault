# Setup Wizard (first-run)

**Purpose:** Documents the first-run administrator creation flow shown when no
admin account exists yet.

## Flow (since 2026-09-22)

1. **Step 1 (Account)** — username + optional fullname; live availability check
   (`GET /auth/check-username`) blocks the step while the name is taken.
2. **Step 2 (Password)** — password + confirmation. Optional **TOTP
   enrolment**: a `theme/Switch` toggle reveals a QR code rendered client-side
   (`qrcode` npm package) from a client-generated base32 secret
   (`crypto.getRandomValues`, 20 bytes), plus a message listing compatible
   apps (Google Authenticator, Microsoft Authenticator, Aegis, Ente Auth,
   2FAS…) with iOS/Android availability. The 6-digit code is validated via the
   stateless `POST /auth/setup/verify-totp` endpoint; the step cannot be left
   until the toggle is off or the code is verified.
3. **Step 3 (Verify)** — server-side summary; "Create" submits `POST /auth/setup`
   with optional `totp_secret` + `totp_code`. The server re-verifies the code
   **before** creating any row (invalid code → clean rejection, nothing
   persisted), then enables 2FA (`enable_2fa`, best-effort) and returns
   `totp_enabled` + `backup_codes`.
4. **Step 4 (Recovery)** — the one-time recovery code is generated and shown
   **after** account creation (mandatory, non-copyable, acknowledged once),
   then — when TOTP was enrolled — a short backup-codes summary, then finish
   (`/login`). There is **no** post-creation 2FA-enrolment screen anymore.

## Backend

- `GET /auth/setup-status` (`handle_setup_status`) — reports whether setup is
  still required (`is_first_run`/`is_setup_valid`). The desktop redirects to the
  wizard while `is_first_run` is true.
- `POST /auth/setup` (`handle_setup`) — creates the initial admin account. Uses
  the **single** correct path `/api/auth/setup` (a past bug double-prefixed it to
  `/api/api/auth/setup`, always 404-ing).
- `GET /auth/check-username` (`handle_check_username`) — live username availability.
- `POST /auth/setup/verify-totp` (`handle_setup_verify_totp`) — stateless,
  first-run-only pre-creation TOTP validation (`{ secret, code }` → `{ valid }`),
  backed by `AuthService::verify_totp_code_with_secret` (window −1..=1 time step).

## Frontend

- `frontend/src/routes/SetupWizard.svelte` — whole wizard: stepper, gating,
  client-side TOTP secret + QR generation (`qrcode`), `verifySetupTotp` call at
  the password step, recovery-code screen after creation, backup-codes summary.
- `frontend/src/lib/services/api.ts`: `getSetupStatus`, `setupAdmin`,
  `checkUsername`, `verifySetupTotp`.
- `src/setup.rs` `SetupService`; models in `src/storage/models.rs`
  (`SetupRequest` + `totp_secret`/`totp_code`, `SetupStatusResponse`).

## Invariants

- The wizard is only reachable before the first admin exists; afterwards
  `/auth/setup-status` reports `is_first_run: false` (and
  `/auth/setup/verify-totp` returns 403).
- The TOTP code is verified **twice** server-side-path: at the password step
  (stateless UX check) and inside `handle_setup` before any row is written.
- The recovery code remains generated **after** account creation, shown once,
  non-copyable; only its hash is stored.
- After account creation the "Previous" button is disabled (a previous bug left
  it active post-creation).
- Must cover all four locales for the TOTP-enrolment step and sudo prerequisite
  banner (FR/DE/ES were missing these strings; now present).
- **Editable since 2026-09-12** — `CLAUDE.md`'s frozen-area registry is gone, so
  `SetupWizard.svelte`, the `setup*` api.ts methods, `src/handlers/auth.rs`'s
  setup handlers, `src/setup.rs` and the `setup.*` i18n keys are open to edits.
  The invariants above stay binding: a change keeps the Rust integration +
  Playwright E2E suites green.

## Related

- [Authentication](./auth.md)
- [API Contract](../API.md)
