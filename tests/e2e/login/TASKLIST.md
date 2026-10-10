# Login / Authentication — Test Tasklist

> Last validated: 2026-09-01 · Firefox · full re-run — Login E2E 91 `test(` + 3 `test.slow` (94 runtime), backend all green
> (count re-verified 2026-10-10; the spec was edited 2026-09-23 and not re-run since)

## Summary

| Layer | File | Tests | Status |
|-------|------|-------|--------|
| Backend unit | `src/auth/tests.rs` + `src/auth/service.rs` (`base32_totp_tests`) | 9 | ✅ 9/9 |
| Backend integration | `tests/login_integration.rs` | 66 | ✅ 66/66 |
| Frontend E2E | `tests/e2e/login/login.spec.ts` | 91 `test(` + 3 `test.slow` = 94 runtime | ✅ last green 2026-09-01 |
| Host-mutating (opt-in, `#[ignore]`) | `tests/login_pam_integration.rs` | 3 | ✅ 3/3 as root |
| **Total** | | **171** | ✅ |

The opt-in PAM suite (run as root) found the "log in with your host Linux
account" path broken **three ways** — all three fixed and verified end-to-end
against the release binary running as an unprivileged `shadow`-group service
account. See the "Host-mutating opt-in tests" section.

Setup-wizard concurrency is covered by `tests/setup_integration.rs`
(`test_concurrent_setup_only_one_succeeds`).

The auth service and handlers keep the **engineering invariants** in
`CLAUDE.md` ("Auth, sessions, recovery codes") — no frozen-area registry
since 2026-09-12. These tests only add coverage — no production file was
changed. If a test starts failing because of a real product change, stop and
report it rather than loosening the assertion.

---

## Backend — Unit Tests (`src/auth/`)

Pure-logic tests, no DB / HTTP / filesystem. Run: `cargo test auth::`

### `src/auth/tests.rs` (`auth_tests`)
- [x] `test_password_hashing` — Argon2 produces a `$argon2…` string
- [x] `test_password_verification` — correct password verifies, wrong one fails
- [x] `test_token_generation` — 32-char alphanumeric token
- [x] `test_username_validation` — alphanumeric + underscore only
- [x] `test_session_token_validation` — expiry comparison logic
- [x] `test_role_validation` — `admin` / `user` / `viewer` only

### `src/auth/service.rs` (`base32_totp_tests`)
- [x] `decode_base32_round_trips_every_alphabet_character`
- [x] `decode_base32_rejects_digits_outside_2_to_7`
- [x] `totp_matches_known_rfc6238_test_vector`

---

## Backend — Integration Tests (`tests/login_integration.rs`)

In-memory SQLite per test, all migrations, Axum router + `tower::oneshot`.
Run: `cargo test --test login_integration`

### 1. `POST /api/auth/login`
- [x] `test_login_success` — token + username in the `{success,data}` envelope
- [x] `test_login_wrong_password` — 401, generic "Invalid username or password"
- [x] `test_login_user_not_found` — 401
- [x] `test_login_empty_credentials` — 4xx
- [x] `test_login_empty_body` — 4xx
- [x] `test_login_json_malformed` — 4xx
- [x] `test_login_missing_password_field_rejected` — serde rejects, 4xx
- [x] `test_login_ignores_unknown_extra_fields` — bogus `is_admin:true` in body ignored
- [x] `test_login_username_is_case_sensitive` — exact-match only (documents behaviour)
- [x] `test_login_wrong_password_does_not_leak_user_existence` — same status + message
- [x] `test_login_remember_me_sets_longer_session` — distinct sessions issued
- [x] `test_remember_me_grants_a_longer_lived_session` — `expires_at` compared in DB
- [x] `test_login_returns_is_admin_for_admin_user`
- [x] `test_login_returns_false_is_admin_for_regular_user`
- [x] `test_login_locks_after_max_attempts` — rate limiter kicks in
- [x] `test_login_locked_account_is_rejected` — `locked=1` → 401 "Account is locked"
- [x] `test_login_returns_csrf_token` — CSRF token present
- [x] `test_login_csrf_token_differs_from_session_token`
- [x] `test_concurrent_sessions_are_both_valid` — 3 logins, all tokens usable

### 2. `GET /api/auth/me`
- [x] `test_me_valid_token` — returns the caller's username
- [x] `test_me_no_token` — 4xx
- [x] `test_me_invalid_token` — 4xx
- [x] `test_me_returns_full_profile_fields` — fullname / email / role / two_fa_enabled / avatar_url

### 3. `POST /api/auth/logout`
- [x] `test_logout_valid_token` — 200
- [x] `test_logout_invalid_token` — no crash (garbage token rejected)
- [x] `test_me_after_logout_fails` — token dead after logout
- [x] `test_logout_only_invalidates_the_calling_session` — session B survives A's logout

### 4. `POST /api/auth/forgot-password`
- [x] `test_forgot_password_existing_user` — generic 200 message
- [x] `test_forgot_password_nonexistent_user` — identical response (no enumeration)
- [x] `test_forgot_password_empty_body` — handled
- [x] `test_forgot_password_rate_limited_after_three_requests` — 4th request rejected

### 5. `POST /api/auth/reset-password`
- [x] `test_reset_password_invalid_token` — rejected
- [x] `test_reset_password_weak_password` — rejected
- [x] `test_reset_password_empty_body` — 4xx
- [x] `test_reset_password_full_flow` — request → real token → reset → old pw fails, new pw works
- [x] `test_reset_password_token_is_single_use` — reuse rejected
- [x] `test_reset_password_invalidates_existing_sessions` — pre-existing session token dies

### 6. `POST /api/auth/2fa/verify` (real 2FA, verified via backup codes)
- [x] `test_login_with_2fa_enabled_returns_requires_2fa` — plain login returns `requires_2fa`, no token
- [x] `test_2fa_verify_with_valid_backup_code_succeeds` — session issued
- [x] `test_2fa_verify_with_invalid_code_fails` — 401
- [x] `test_2fa_verify_backup_code_is_single_use` — second use of the same code rejected
- [x] `test_2fa_verify_wrong_password_with_valid_backup_code` — 401
- [x] `test_2fa_verify_nonexistent_user` — 401
- [x] `test_2fa_verify_no_session` — 401
- [x] `test_2fa_verify_without_2fa_enabled` — authenticates normally, code ignored
- [x] `test_2fa_verify_wrong_password` — 401
- [x] `test_2fa_verify_empty_body` — 4xx

### 7. Full flow
- [x] `test_full_auth_flow` — login → /me → logout → /me fails

### 8. CSRF middleware (real `csrf_middleware` layer, as wired in `web/mod.rs`)
- [x] `test_csrf_rejects_authenticated_post_without_token` — 403
- [x] `test_csrf_rejects_wrong_token_and_accepts_matching_token` — 403 then 200
- [x] `test_csrf_does_not_touch_get_requests` — GET /me still works

### 9. Session & token expiry, cleanup jobs
- [x] `test_me_rejects_an_expired_session_token`
- [x] `test_refresh_session_extends_expiry` — `expires_at` rewritten to now + lifetime
- [x] `test_refresh_session_unknown_token_errors`
- [x] `test_cleanup_expired_sessions_removes_only_expired_rows`
- [x] `test_cleanup_expired_reset_tokens`
- [x] `test_reset_password_with_expired_token_is_rejected`

### 10. Password history
- [x] `test_reset_password_rejects_a_recently_used_password` — `PasswordReused`

### 11. Reset lookup is username-only (known stub)
- [x] `test_forgot_password_by_email_does_not_produce_a_working_token` — generic 200, no token persisted

### 12. Concurrency
- [x] `test_concurrent_wrong_logins_still_lock_the_account` — burst of failures cannot race the lockout open

### 13. HTTP rate-limit middleware (`rate_limit_middleware`, keyed by client IP)
- [x] `test_rate_limit_middleware_blocks_after_the_limit` — Nth+1 request → 429
- [x] `test_rate_limit_middleware_is_per_ip` — separate IPs keep separate budgets
- [x] `test_rate_limit_state_window_resets_after_expiry` — sliding window rolls over

### 14. Hardware-change detection on `/setup-status`
- [x] `test_setup_status_reports_hardware_changed_when_hash_file_mismatches` — stale `.system_hash` → `hardware_changed: true`
- [x] `test_setup_status_no_hardware_change_when_hash_matches`

### 15. Email delivery path
- [x] `test_email_delivery_fails_cleanly_without_an_mta` — `send_email` shells out to
  `mutt`/`sendmail`/`msmtp`; with none installed it returns `Err`, never a silent success

---

## Backend — Host-mutating opt-in tests (`tests/login_pam_integration.rs`)

`#[ignore]` — need root + `useradd`/`userdel`/`chpasswd` + the `PAM` Python module.
Each self-skips (does not fail) when the environment can't run it.

```
sudo -E cargo test --test login_pam_integration -- --ignored --nocapture
```

Ran as root on the dev box 2026-09-01. **These tests found the "log in with your
host Linux account" feature broken in three independent ways — all fixed:**

- [x] ✅ `pam_login_succeeds_for_a_real_linux_account` — regression guard for **fix #1**:
  `authenticate_with_pam` ran `ok = auth.authenticate(); sys.exit(0 if ok else 1)`;
  PyPAM returns `None` on success (raises on failure), so `ok` was always falsy →
  every host login rejected even with the right password. Fixed in
  `src/auth/service.rs` — treat "no exception raised" as success.
- [x] ✅ `pam_user_in_sudo_group_is_promoted_to_admin` — regression guard for **fix #2**:
  `create_session` for a host account failed with `FOREIGN KEY constraint failed`
  (`sessions.username` → `users.username`, no row for a PAM user). Fixed in
  `src/auth/service.rs`: `ensure_pam_user_row` upserts a minimal `users` row
  (sentinel hash `!pam-provisioned`, `role_id` re-synced to host group membership)
  on every PAM login. `authenticate` recognises the sentinel and falls through to
  PAM on subsequent logins. Also asserts a 2nd login still works and that
  demotion out of sudo/wheel downgrades the mirrored role.
- [x] ✅ `configure_sudoers_produces_a_visudo_valid_file` — ran as root, self-skipped
  as designed (no `HOMEVAULT_DESTRUCTIVE_ROOT_TESTS=1`); wiring verified.

**Fix #3** (`installer/install.sh`): the systemd unit runs the service as the
unprivileged `homevault` user; `pam_unix` → `unix_chkpwd` then refuses to verify
*another* user's password unless the caller can read `/etc/shadow`. Added
`usermod -aG shadow homevault` next to the existing `usermod -aG docker`.

**End-to-end verification (2026-09-01):** the release binary run as a throwaway
unprivileged account in the `shadow` group — a host user in `sudo` logs in via
`POST /api/auth/login` → `{"success":true,"is_admin":true}`; wrong password →
401; `/api/auth/me` returns the mirrored `role:"admin"` profile; removing the
user from `sudo` and logging in again → `is_admin:false`.

---

## Frontend — E2E Tests (`tests/e2e/login/login.spec.ts`)

Runs against a live server. Each test detects the page state (login form /
setup wizard / connection error) and adapts.

Run: `BASE_URL=http://localhost:8080 npx playwright test tests/e2e/login/login.spec.ts --project=firefox`

### 1. Page load (4)
- [x] 1.1 spinner while fetching setup status
- [x] 1.2 one of login / wizard / error (never blank)
- [x] 1.3 brand header — title `HomeVault`, non-empty subtitle
- [x] 1.4 icon box visible and sized

### 2. Form fields (4)
- [x] 2.1 username: type=text, required, autocomplete=username
- [x] 2.2 password: type=password, required, autocomplete=current-password
- [x] 2.3 both fields have placeholders
- [x] 2.4 submit button has correct initial label

### 3. Validation — native `required` first line of defence (5)
- [x] 3.1 empty submit fires no login request; username reports `valueMissing`
- [x] 3.2 username only → password reports `valueMissing`
- [x] 3.3 password only → username reports `valueMissing`
- [x] 3.4 invalid credentials → error modal, dismiss button clears it
- [x] 3.5 after dismissing, the form is interactive and re-submittable

### 4. Password toggle (3)
- [x] 4.1 toggles password ↔ text
- [x] 4.2 aria-label changes with state
- [x] 4.3 toggle is keyboard-reachable

### 5. Remember me (4)
- [x] 5.1 present, starts unchecked
- [x] 5.2 toggles on/off
- [x] 5.3 label associated via for/id
- [x] 5.4 clicking the label toggles it

### 6. Login submission (5)
- [x] 6.1 wrong credentials → error modal with message
- [x] 6.2 fields disabled during the request
- [x] 6.3 submit button shows a spinner while loading
- [x] 6.4 form re-enables after a failed login
- [x] 6.5 submit button re-enables after the request completes

### 7. Forgot password (6)
- [x] 7.1 link visible
- [x] 7.2 opens the panel with a username field
- [x] 7.3 panel has a back button
- [x] 7.4 submitting shows a confirmation (no enumeration)
- [x] 7.5 back returns to the login form
- [x] 7.6 field pre-fills with the current username

### 8. Reset password (URL token) (6)
- [x] 8.1 `/login?reset_token=X` shows the reset form
- [x] 8.2 reset form has a password visibility toggle
- [x] 8.3 mismatched passwords → **visible inline `.hv-reset-error`** (was silent)
- [x] 8.4 back button drops the token from the URL
- [x] 8.5 fields have `autocomplete=new-password`
- [x] 8.6 invalid/expired token → **visible `.hv-reset-error`** ("invalid/expired/used")

**Bug fixed (frozen `Login.svelte`, user-authorised):** `handleResetPasswordSubmit`
had an empty `else {}` and `catch {}` — a rejected reset (bad/expired/used token,
weak or reused password, network error) showed the user *nothing*. Now a
`resetError` is set and rendered inline, mapped to a specific localized message
(7 new `login.reset_*` keys × 4 languages).

### 9. 2FA — live TOTP enrolment on the seeded admin (`describe.serial`) (8)
- [x] 9.1 valid credentials for a 2FA account reveal 6 code inputs
- [x] 9.2 code inputs are numeric one-time-code fields
- [x] 9.3 2FA step shows heading + description
- [x] 9.4 a wrong code → error modal, inputs cleared
- [x] 9.5 Back link returns to the username/password form
- [x] 9.6 a correct TOTP code completes login and leaves `/login`
- [x] 9.7 typing digits auto-advances focus across the boxes
- [x] 9.8 Backspace on an empty box moves focus to the previous one
- `beforeAll` enrols a secret (session + CSRF headers); `afterAll` removes it.

### 10. Connection error (2)
- [x] 10.1 WifiOff icon, message, retry button
- [x] 10.2 retry re-runs the setup-status check

### 11. Setup wizard (1)
- [x] 11.1 wizard, when active, hides the login form

### 12. Responsive (5) — 360 / 390 / 768 / 1024 / 1920 px
- [x] form fits the viewport, no horizontal overflow, key fields visible

### 13. Accessibility (5)
- [x] 13.1 inputs have labels
- [x] 13.2 required fields carry `required`
- [x] 13.3 no duplicate IDs
- [x] 13.4 no negative tabindex on interactive elements
- [x] 13.5 buttons are keyboard-focusable

### 14. Performance (5)
- [x] 14.1 loads < 5 s
- [x] 14.2 interactive DOM node count < 2000 (excludes the inlined icon sprite)
- [x] 14.3 resource requests < 50
- [x] 14.4 no heap leak across 5 navigations
- [x] 14.5 first paint < 3 s

### 15. Security (7)
- [x] 15.1 password field is `type=password`
- [x] 15.2 password empty on load
- [x] 15.3 password never appears in the URL
- [x] 15.4 no `session-token` in localStorage after a failed login
- [x] 15.5 no CSRF token stored after a failed login
- [x] 15.6 XSS in the username does not execute
- [x] 15.7 very long input does not break the page

### 16. Keyboard (3)
- [x] 16.1 forward Tab order: username → password → toggle → remember → forgot → submit
- [x] 16.2 Enter in the password field submits
- [x] 16.3 the error modal can be closed (Escape or button) and reopened

### 17. Console & network (3)
- [x] 17.1 no uncaught JS errors on load
- [x] 17.2 no `console.error` on a clean load
- [x] 17.3 a login attempt makes ≤ 5 API calls

### 18. Edge cases (5)
- [x] 18.1 unicode username
- [x] 18.2 whitespace-only username
- [x] 18.3 rapid double-click does not flood the endpoint
- [x] 18.4 works after `localStorage.clear()`
- [x] 18.5 unknown route shows the login page

### 19. Memory regression (`test.slow()`) (3)
- [x] 19.1 repeated failed logins do not leak heap (> 3 MB)
- [x] 19.2 error-modal cycle ×5 does not accumulate DOM nodes
- [x] 19.3 password toggle ×20 does not leak

### 20. i18n — the saved `hv-language` drives the rendered strings (4)
- [x] 20.1 French — subtitle / submit / remember label
- [x] 20.2 German — submit button
- [x] 20.3 Spanish — submit button
- [x] 20.4 no raw `login.*` keys leak into the form

### 21. Error-message specificity (`test.slow()`) (2)
- [x] 21.1 wrong credentials → "incorrect username or password"
- [x] 21.2 repeated failures walk through **3 distinct localized messages**:
      bad credentials → "account has been locked" → "too many login attempts"
      (fresh username per run)

### 22. Successful login (1)
- [x] 22.1 valid admin credentials leave `/login` and store a `session-token`
      (skips if the seeded admin still has 2FA from an interrupted run)

### 23. Non-admin login (`describe.serial`, 1)
- [x] 23.1 a valid **non-admin** login is refused at the UI with the
      "access restricted to administrators" modal; stays on `/login`; the
      transient server session is cleared again (no `session-token` left).
      `beforeAll` creates a role-1 account via the real `POST /api/users`
      (admin session); `afterAll` deletes it.

### 24. Reset-password success screen (`describe.serial`, 1)
- [x] 24.1 `forgot-password` → read the real token from the server's SQLite file
      (`E2E_DATA_DIR/data/homevault.db`, via `node:sqlite`) → drive
      `/login?reset_token=…` → assert the success screen + "sign in" button →
      confirm the new password authenticates and the old one returns 401.
      Skips if `E2E_DATA_DIR` is unset. Uses a throwaway role-1 account.

---

## How to Run

```bash
# Backend unit
cargo test auth::

# Backend integration (in-memory SQLite, no server)
cargo test --test login_integration
cargo test --test login_integration test_2fa_verify_with_valid_backup_code_succeeds

# Frontend E2E (needs a live server; seed it with a completed setup first)
BASE_URL=http://localhost:8080 npx playwright test tests/e2e/login/login.spec.ts --project=firefox
npx playwright test tests/e2e/login/login.spec.ts -g "9.6"

# The 2FA / non-admin / reset-success suites need the seeded admin + the server's
# data directory (for the reset test's DB read). Override if different:
E2E_ADMIN_USER=e2eadmin E2E_ADMIN_PASS=E2eSecret123 \
E2E_DATA_DIR=/var/lib/homevault \
  npx playwright test tests/e2e/login/login.spec.ts --project=firefox

# Host-mutating opt-in tests (root + PAM + useradd; run on a disposable box)
sudo -E cargo test --test login_pam_integration -- --ignored --nocapture
```

## Notes / gotchas

- **`required` attributes**: the username/password fields block empty/partial
  submits natively — the JS `showLoginError('login.validation.*')` branch in
  `handleLogin` is effectively unreachable from the button. Section 3 asserts
  the native path, not the modal.
- **Slow failed logins**: a wrong password for a real DB user runs Argon2
  (deliberately slow). A wrong *username* now fails fast — `authenticate` gates
  the PAM fallback behind an `id`-based existence check, so garbage usernames no
  longer pay for a `python3` spawn + `pam_unix`'s ~2 s auth-failure delay. Tests
  that submit a bad login wait for the modal (`toBeVisible({timeout})`), never a
  fixed `waitForTimeout`. Loop-heavy tests are `test.slow()`; full run ~11 min.
- **2FA E2E state**: the 2FA suite enrols and then removes a TOTP secret on the
  seeded admin. If `afterAll` is interrupted the account keeps 2FA — start from
  a fresh data directory, or disable it manually.
- **`x-csrf-token`**: state-changing auth endpoints (`/api/auth/2fa/setup`) sit
  behind CSRF middleware; API-level calls must send both `x-session-token` and
  the paired `x-csrf-token` from the login response.
- **Persistent server state in E2E**: the failed-attempt counter and TOTP
  enrolment live in the server DB, not the browser context. Tests that hammer
  the lockout use a per-run username (`spec_rl_${Date.now()}`); reruns against
  the *same* data directory otherwise inherit locks. `cargo test` suites are
  immune (fresh in-memory DB each test).

## Coverage of the previously-open gaps

| Gap | Status |
|---|---|
| HTTP rate-limit middleware | ✅ integration §13 (real `rate_limit_middleware` layer) |
| `hardwareChanged` re-setup path | ✅ integration §14 (stale `.system_hash` file) |
| Frontend "not admin" error | ✅ E2E §23 (viewer created via real `POST /api/users`) |
| Reset-password success screen (E2E) | ✅ E2E §24 (token read from the server's SQLite file) |
| Reset-password **failure** feedback | ✅ E2E §8.3 / §8.6 + **bug fixed** — silent `else {}` / `catch {}` in `handleResetPasswordSubmit` now shows an inline localized error |
| Error-message specificity (`account_locked` etc.) | ✅ E2E §21.2 asserts the walk-through of 3 distinct localized messages |
| Email delivery | ⚠️ §15 — the app has **no SMTP client**; it shells out to
  `mutt`/`sendmail`/`msmtp`. Real delivery needs one of those installed +
  configured on the host, exactly like PAM. §15 asserts the no-MTA failure path;
  a full send test belongs in the same privileged-CI bucket as the PAM tests. |
| PAM host-account login | ✅ opt-in `#[ignore]` tests, run as root — found **and
  fixed** 3 bugs (see the "Host-mutating opt-in tests" section); verified
  end-to-end against the release binary. Not run in normal `cargo test`. |
| sudoers generation | ✅ wiring covered; the destructive body is opt-in behind
  `HOMEVAULT_DESTRUCTIVE_ROOT_TESTS=1`. |

## Still not covered (and why)

- **Real SMTP send** — the app has no SMTP client (it shells out to
  `mutt`/`sendmail`/`msmtp`); a full send test needs an MTA on the host, same
  bucket as the PAM tests. §15 covers the no-MTA failure path.
- **PAM login in normal `cargo test`** — the tests exist and pass, but are
  `#[ignore]` (need root + `useradd`); run them explicitly in a privileged CI.
- **Chromium** — only Firefox is validated (matches `tests/e2e/setup`).
