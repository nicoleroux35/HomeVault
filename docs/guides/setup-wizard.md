# Setup Wizard (first-run)

**Purpose:** Documents the first-run administrator creation flow shown while no
admin account exists, the route guard that decides which screen the shell
renders, and the hardware-change recovery flow.

## State model (2026-10-08) — the database is the source of truth

"Has this appliance been configured?" is answered from the **database**, not
from a marker file:

- `storage::SetupStateRepository::has_admin()` — a row in `users` with
  `role_id = 2` (`ADMIN_ROLE_ID`) exists. This is the canonical check.
- `setup_state` (migration `037_setup_state.sql`) — a singleton row holding the
  `system_hash` / `machine_id` fingerprint recorded at setup time. It carries
  only what `users` cannot. See `src/storage/setup_state_repo.rs`.

Before this change the answer came from `.setup_complete` / `.system_hash`
files under `base_dir()`, while the admin account lived in the database. The two
could disagree — losing the data directory while the DB survived re-opened the
Wizard even though an admin still existed, and the reverse dead-ended the
Wizard on "Setup already complete" after a hardware change. Neither marker file
exists any more; the only filesystem artifact left is the `.setup_lock`
concurrency lock (`SetupService::mark_setup_in_progress`, `O_EXCL`).

`GET /auth/setup-status` derives everything from those two sources:

| Field | Meaning |
|---|---|
| `is_first_run` | no admin row exists |
| `is_setup_valid` | an admin exists and the recorded fingerprint matches this host |
| `hardware_changed` | a fingerprint is recorded and it no longer matches |
| `needs_recovery` | `is_first_run == false && hardware_changed` — show the recovery screen |
| `server_machine_id` | `/etc/machine-id`, or a hostname fallback |
| `sudo_available` | `which sudo` succeeded (terminal capability banner) |

### Durability — surviving a system-disk reinstall

Both the accounts and the setup state live in
`{DATA_DIRECTORY}/data/homevault.db` (default `/var/lib/homevault/data`), so
pointing `DATA_DIRECTORY` at a persistent volume (a storage pool mount, an
external drive) makes the whole identity — accounts, setup fingerprint,
sessions — survive a reinstall of the system disk. With an empty database and
no marker files, setup simply runs again.

The Umbrel-style *reboot-during-setup* RAID flow (write credentials to the
pool, reboot, create the account on the next boot) is **not** implemented:
HomeVault's wizard has no storage-selection step, and pools are created from
the Storage app *after* setup. Deferred, tracked here rather than silently
assumed.

## Flow (wizard, 4 steps)

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

`POST /auth/setup` then records the fingerprint via
`SetupStateRepository::mark_complete` and drops the lock. Recording the
fingerprint is best-effort: the admin row is already committed, and the
fingerprint only exists to detect a moved system.

## Frontend route guard — `lib/stores/appGate.ts` (2026-10-08)

The shell decides **once** which screen to render, the equivalent of Umbrel's
`EnsureUserExists` / `EnsureUserDoesntExist`:

- `appGate.refresh()` reads `GET /auth/setup-status`, then:
  `is_first_run` → `setup`; `needs_recovery` → `recovery`; otherwise
  `authStore.checkAuth()` → `app` or `login`; a thrown status read → `error`.
- `App.svelte` renders `SetupWizard` / `Login recoveryMode` /
  `Login connectionError` / `Login` / `Desktop` from `$appGate.route`, and
  keeps the address bar in sync.
- `Login.svelte` no longer fetches the setup status or renders
  `SetupWizard` inline. After a successful (admin) sign-in it calls
  `appGate.enterApp()`; `Desktop.svelte`'s logout calls `appGate.signOut()`.

## Hardware-change recovery

A changed fingerprint (VM clone, hostname/OS change, restored database) with an
existing admin routes to `Login recoveryMode`, **not** the Wizard:

- The login screen shows a warning banner and requires the admin to sign in
  again — step-up authentication for a fingerprint change.
- On success it calls `POST /auth/setup/rebaseline` (admin-authenticated),
  which rewrites `setup_state` with the current fingerprint and logs an event,
  then enters the desktop. `needs_recovery`/`hardware_changed` go false.

This replaces the previous dead-end where the Wizard was shown for
`hardware_changed` but `/auth/setup` correctly answered "Setup already
complete".

## Local-network restriction (2026-10-08)

The bootstrap endpoints are public by necessity — they exist because no admin
exists yet — so `middleware::setup_guard` restricts them to clients whose TCP
peer is loopback or in a private / link-local / ULA range:

- Guarded: `POST /auth/setup`, `POST /auth/setup/verify-totp`,
  `GET /auth/check-username`.
- **Not** guarded: `GET /auth/setup-status` (the login screen reads it from
  anywhere) and `POST /auth/setup/rebaseline` (already admin-authenticated).
- The peer address comes from `ConnectInfo`, **never** from
  `X-Forwarded-For`/`X-Real-IP` (client-settable → forgeable).
- `HOMEVAULT_SETUP_ALLOW_REMOTE=1` (or `true`/`yes`) lifts the restriction for
  a deliberate remote first-run setup.

Rejected requests get `403 {"success":false,"message":"First-run setup is only
available from the local network"}`.

## Backend

- `GET /auth/setup-status` (`handle_setup_status`) — DB-derived status (table above).
- `POST /auth/setup` (`handle_setup`) — creates the initial admin. Guarded by
  `has_admin()`, not a marker file; the `O_EXCL` lock still serialises
  concurrent calls. Uses the **single** correct path `/api/auth/setup`.
- `GET /auth/check-username` (`handle_check_username`) — live username availability.
- `POST /auth/setup/verify-totp` (`handle_setup_verify_totp`) — stateless,
  first-run-only pre-creation TOTP validation (`{ secret, code }` → `{ valid }`),
  backed by `AuthService::verify_totp_code_with_secret` (window −1..=1 time step).
- `POST /auth/setup/rebaseline` (`handle_setup_rebaseline`) — admin-only
  fingerprint re-baseline for the recovery flow.

## Frontend

- `frontend/src/App.svelte` — gate-driven shell rendering.
- `frontend/src/lib/stores/appGate.ts` — the guard (`refresh` / `enterApp` /
  `signOut`).
- `frontend/src/routes/SetupWizard.svelte` — the wizard; stepper, gating,
  client-side TOTP secret + QR generation (`qrcode`), `verifySetupTotp` call at
  the password step, recovery-code screen after creation, backup-codes summary.
- `frontend/src/routes/Login.svelte` — login form, 2FA step, recovery-code
  panel; `recoveryMode` banner + re-baseline, `connectionError` screen.
- `frontend/src/lib/services/api.ts`: `getSetupStatus` (throws on failure,
  never faked as first-run), `setupAdmin`, `checkUsername`, `verifySetupTotp`,
  `rebaselineSetup`.
- `src/setup.rs` `SetupService`; `src/storage/setup_state_repo.rs`;
  `src/middleware/setup_guard.rs`; models in `src/storage/models.rs`.

## Invariants

- `is_first_run` is derived from the admin row in `users`, never from a file.
- The wizard is only reachable before the first admin exists; afterwards
  `/auth/setup/verify-totp` returns 403 and `/auth/setup` answers
  "Setup already complete".
- A hardware change with an admin present routes to the recovery screen, never
  the Wizard; `POST /auth/setup/rebaseline` is the documented way out.
- The TOTP code is verified **twice** server-side-path: at the password step
  (stateless UX check) and inside `handle_setup` before any row is written.
- The recovery code remains generated **after** account creation, shown once,
  non-copyable; only its hash is stored.
- After account creation the "Previous" button is disabled.
- The `.setup_lock` `O_EXCL` lock is kept; it is the only filesystem state.
- The bootstrap endpoints stay local-network-only unless
  `HOMEVAULT_SETUP_ALLOW_REMOTE` is set.
- Must cover all four locales for the TOTP-enrolment step, the sudo prerequisite
  banner and the hardware-recovery banner (FR/DE/ES were missing these strings;
  now present).
- **Editable since 2026-09-12** — `CLAUDE.md`'s frozen-area registry is gone, so
  `SetupWizard.svelte`, the `setup*` api.ts methods, `src/handlers/auth.rs`'s
  setup handlers, `src/setup.rs` and the `setup.*` i18n keys are open to edits.
  The invariants above stay binding: a change keeps the Rust integration +
  Playwright E2E suites green.

## Related

- [Authentication](./auth.md)
- [API Contract](../API.md)
- [Backend overview](../BACKEND.md)
