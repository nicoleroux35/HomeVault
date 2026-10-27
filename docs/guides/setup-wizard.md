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
| `system_language` | the host's POSIX locale mapped to `en`/`fr`/`de`/`es` (English when unsupported) — the wizard's initial language |

### Durability — surviving a system-disk reinstall

Both the accounts and the setup state live in
`{DATA_DIRECTORY}/data/homevault.db` (default `/var/lib/homevault/data`), so
pointing `DATA_DIRECTORY` at a persistent volume (a storage pool mount, an
external drive) makes the whole identity — accounts, setup fingerprint,
sessions — survive a reinstall of the system disk. With an empty database and
no marker files, setup simply runs again.

The *reboot-during-setup* RAID flow of the reference design (write credentials to the
pool, reboot, create the account on the next boot) is **not** implemented, and
the wizard deliberately has **no storage, RAID or restore step**: the appliance
is installed by script on an existing OS, pools/arrays are created from the
Storage app and a backup is restored from the Backup app — all *after* setup.
What the wizard does keep from that flow is its own reboot-safe progress (see
*Progress persistence*). Deferred, tracked here rather than silently assumed.

## Flow (wizard, 4 steps)

The wizard is **reboot-safe**: the current step is persisted server-side (see
*Progress persistence* below), so a reboot or a closed browser resumes where
the operator left off.

1. **Step 1 (Welcome)** — what HomeVault is; no input.
2. **Step 2 (Account)** — username + optional fullname; live availability check
   (`GET /auth/check-username`) blocks the step while the name is taken.
3. **Step 3 (Password)** — password + confirmation. Optional **TOTP
   enrolment**: a `theme/Switch` toggle reveals a QR code rendered client-side
   (`qrcode` npm package) from a client-generated base32 secret
   (`crypto.getRandomValues`, 20 bytes), plus a message listing compatible
   apps (Google Authenticator, Microsoft Authenticator, Aegis, Ente Auth,
   2FAS…) with iOS/Android availability. The 6-digit code is validated via the
   stateless `POST /auth/setup/verify-totp` endpoint; the step cannot be left
   until the toggle is off or the code is verified.
4. **Step 4 (Verify & create)** — server-side summary (account); "Create"
   submits `POST /auth/setup` with optional
   `totp_secret` + `totp_code`. The server re-verifies the code **before**
   creating any row (invalid code → clean rejection, nothing persisted), then
   enables 2FA (`enable_2fa`, best-effort) and returns `totp_enabled` +
   `backup_codes`. The one-time recovery code is then generated and shown
   **after** account creation (mandatory, non-copyable, acknowledged once),
   followed — when TOTP was enrolled — by a short backup-codes summary, then
   finish (`/login`). There is **no** post-creation 2FA-enrolment screen.

### Interface language — detected, never asked (2026-10-17)

The wizard has **no language step**. `languageStore`
(`frontend/src/lib/stores/i18n.ts`) resolves the dictionary in this order:

1. an explicit saved choice (`localStorage['hv-language']`, written only by
   Settings);
2. the browser/OS locale (`navigator.language`, which follows the OS);
3. the **host's own POSIX locale**, reported by `GET /auth/setup-status` as
   `system_language` and applied through `languageStore.applySystemLanguage()`
   from `appGate.refresh()` — `SetupService::detect_system_language()` reads
   `LC_ALL` → `LC_MESSAGES` → `LANG` → `LANGUAGE`, then `/etc/default/locale`;
4. English.

Anything unsupported (`it_IT`, `ja_JP`, `C`, `POSIX`, …) falls back to English.
Settings (`settings.language_label`) is the only place a preference is stored,
so the detection never pins a language permanently.

## Progress persistence (reboot-safe)

The step and the non-secret choices live in a **singleton `setup_progress` row**
(migration `040_setup_progress.sql`, `src/storage/setup_progress_repo.rs`):

- `GET /auth/setup/progress` → `{ step, data }`; `PUT /auth/setup/progress`
  (`{ step, data }`) saves it. Both are behind the same first-run guard as the
  other bootstrap routes and return **403 once an admin exists**.
- `data` is always an empty object now: no wizard choice is worth remembering
  (the interface language is detected from the OS). Secrets — the password and
  the TOTP secret — are **never** part of this payload.
- On load the wizard resumes at the saved step and shows a "picked up where you
  left off" banner; the banner clears on the next step change.

`POST /auth/setup` then records the fingerprint via
`SetupStateRepository::mark_complete` and drops the lock. Recording the
fingerprint is best-effort: the admin row is already committed, and the
fingerprint only exists to detect a moved system.

## Frontend route guard — `lib/stores/appGate.ts` (2026-10-08)

The shell decides **once** which screen to render, the standard
route-guard pattern:

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

## Bootstrap endpoint hardening (2026-10-17)

The bootstrap endpoints are public by necessity — they exist because no admin
exists yet — so `middleware::setup_guard` applies **three** independent checks,
in this order:

### 1. Same-origin (defeats cross-site CSRF)

The CORS layer (`src/web/mod.rs`) is permissive (`allow_origin(Any)`), so the
local-network check alone is not enough: the operator's browser *is* on the LAN,
and the mDNS name (`homevault.local`) is predictable, so any website the
operator visits could `POST /api/auth/setup` and claim the first admin. The
guard therefore refuses:

- an `Origin` header that does not match the request's `Host` header or HTTP/2
  `:authority` pseudo-header (authority only, scheme-agnostic);
- `Origin: null` (sandboxed iframe, `file://`);
- `Sec-Fetch-Site` other than `same-origin` / `none` — so `cross-site` **and**
  `same-site` (a sibling subdomain is a different origin) are refused;
- requests with **neither** header are allowed: non-browser clients (curl, a
  provisioning script over SSH) send neither and are already covered by the
  network check and the lock.

Rejection: `403 {"success":false,"message":"First-run setup is only available
from HomeVault's own interface"}`. The check applies even with
`HOMEVAULT_SETUP_ALLOW_REMOTE=1` — it protects a different hole.

### 2. Rate limit (per peer)

20 requests per 60 s per peer address, on all guarded routes, before the
handlers run. Exceeding it returns
`429 {"success":false,"message":"Too many setup requests, please wait a
minute"}`. The limiter lives in a `OnceLock` inside `setup_guard.rs` rather
than `AppState`, so the guard stays a self-contained route layer.

### 3. Local network (unchanged)

Clients whose TCP peer is loopback or in a private / link-local / ULA range:

- Guarded: `POST /auth/setup`, `POST /auth/setup/verify-totp`,
  `GET /auth/check-username`.
- **Not** guarded: `GET /auth/setup-status` (the login screen reads it from
  anywhere) and `POST /auth/setup/rebaseline` (already admin-authenticated).
- The peer address comes from `ConnectInfo`, **never** from
  `X-Forwarded-For`/`X-Real-IP` (client-settable → forgeable).
- `HOMEVAULT_SETUP_ALLOW_REMOTE=1` (or `true`/`yes`) lifts only this check.

Rejection: `403 {"success":false,"message":"First-run setup is only available
from the local network"}`.

## The setup lock is self-healing (2026-10-17)

`.setup_lock` (under `base_dir()`) still serialises concurrent setups with
`O_EXCL`, but it now records its owner:

```
pid=<pid>
created_at=<unix seconds>
```

An existing lock is honoured only while it is **live**: the PID still exists
(`/proc/<pid>`, or `kill -0` where `/proc` is absent) *and* the lock is younger
than `SETUP_LOCK_MAX_AGE` (15 min). Anything else — dead PID, older than 15 min,
unreadable, or a legacy lock written by an older build — is **reclaimed**:
`mark_setup_in_progress` removes it and re-creates it under `O_EXCL` (so a
concurrent reclaim still yields exactly one winner).

`SetupService::cleanup_stale_setup_lock()` also runs once at startup, so a
reboot after a crashed first run starts clean even when the stale lock is still
young. Before this, a single interrupted setup dead-ended the wizard forever:
`is_first_run` stayed `true`, the UI kept offering setup, and every submission
answered "Setup is already in progress".

## Backend

- `GET /auth/setup-status` (`handle_setup_status`) — DB-derived status (table above).
- `POST /auth/setup` (`handle_setup`) — creates the initial admin. Guarded by
  `has_admin()`, not a marker file; the `O_EXCL` lock (PID + age, self-healing)
  still serialises concurrent calls. The password goes through
  `SetupService::validate_password` (12 chars, mixed case + digit, no
  compromised password, no 5+ repeated run — see `auth.md` → *Password policy*).
  Uses the **single** correct path `/api/auth/setup`.
- `GET /auth/check-username` (`handle_check_username`) — live username
  availability, **first-run-only** (403 once an admin exists, like
  `verify-totp`/`progress` — post-setup the wizard is gone and the endpoint
  would be a free username oracle).
- `POST /auth/setup/verify-totp` (`handle_setup_verify_totp`) — stateless,
  first-run-only pre-creation TOTP validation (`{ secret, code }` → `{ valid }`),
  backed by `AuthService::verify_totp_code_with_secret` (window −1..=1 time step).
- `POST /auth/setup/rebaseline` (`handle_setup_rebaseline`) — admin-only
  fingerprint re-baseline for the recovery flow.
- `GET/PUT /auth/setup/progress` (`handle_setup_progress_get` / `_put`) —
  reboot-safe wizard progress (step + non-secret choices), 403 once an admin
  exists. Guarded by the same network + same-origin check as the other
  bootstrap routes but **exempt from their 20/min rate budget**: the wizard
  fires one PUT per step navigation, so metering it made the wizard starve its
  own `check-username` calls (the account step then refused to advance).
- The wizard's `checkUsernameAvailability()` treats only a **definite verdict**
  (`success && data`) as an answer: a 429/5xx/network failure fails open (no
  "username already taken" message, step allowed to advance) because
  `POST /auth/setup` revalidates the name server-side. Reading
  `data?.available !== true` alone turned every refusal into a permanent
  "this name is taken" dead end.

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
  `rebaselineSetup`, `getSetupProgress`/`saveSetupProgress`.
- `src/setup.rs` `SetupService`; `src/storage/setup_state_repo.rs`;
  `src/storage/setup_progress_repo.rs`; `src/middleware/setup_guard.rs`; models
  in `src/storage/models.rs`.

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
  non-copyable; only its hash is stored. The screen is **blocking**: a failed
  generation offers a retry and never a "continue", because the code is the
  only offline way back into the account.
- After account creation the "Previous" button is disabled.
- The `.setup_lock` `O_EXCL` lock is kept; it is the only filesystem state. It
  records `pid` + `created_at` and is **self-healing**: a dead PID or an age
  over 15 min makes it reclaimable, and `cleanup_stale_setup_lock()` runs at
  startup. Never go back to a bare marker file that can dead-end the wizard.
- **The bootstrap routes are same-origin + rate-limited.** The CORS layer is
  permissive, so `setup_guard` must keep rejecting a foreign `Origin` /
  `Sec-Fetch-Site` and keep its 20 req/min per-peer limiter. Removing either
  re-opens remote admin takeover from a cross-site page.
- **One password policy.** `SetupService::validate_password` is the only one:
  12 chars minimum, mixed case + digit, no compromised password, no run of 5+
  identical characters. `AuthService::validate_password_strength` delegates to
  it, and the wizard's checklist must list every rule (it is what enables
  "Next").
- **Wizard progress holds no secret.** `setup_progress.data` must stay empty
  of secrets — the password and the TOTP secret can never be added to it.
- **The wizard never asks for a language.** It is detected from the OS/browser
  locale (English when unsupported) and changed in Settings; do not add a
  language step or a language field to `POST /auth/setup`.
- **The wizard has no storage, RAID or restore step.** Pools and arrays are
  created from the Storage app and a backup is restored from the Backup app,
  all after setup; do not add disk detection, a `mkfs`/`zpool` call or a
  restore into the wizard or `/auth/setup`.
- The bootstrap endpoints stay local-network-only unless
  `HOMEVAULT_SETUP_ALLOW_REMOTE` is set; the same-origin and rate-limit checks
  apply regardless.
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
