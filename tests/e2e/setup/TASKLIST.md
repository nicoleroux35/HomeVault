# Setup Wizard — Test Tasklist

> Last validated: 2026-09-01 · Firefox · all layers green
> (E2E count re-verified 2026-10-10: 90 `test(` + 2 `test.slow` = 92 runtime; spec edited 2026-09-28 since)

## Summary

| Layer | File | Tests | Status |
|-------|------|-------|--------|
| Backend unit | `src/setup.rs` | 43 | ✅ 43/43 |
| Backend integration | `tests/setup_integration.rs` | 28 | ✅ 28/28 |
| Frontend E2E | `tests/e2e/setup/setup.spec.ts` | 90 `test(` + 2 `test.slow` = 92 runtime | ✅ last green 2026-09-01 |
| **Total** | | **164** | ✅ |

**2026-09-01 restructure.** The E2E suite had ~40 tests that silently no-op'd:
the first test to click "Create Account" completed first-run setup, and every
later `ensureWizard()`-gated test then hit `if (!isWizard) return`. Fixed:

- The 10 weak/no-op tests in the old sections 7 (Account creation) and 8 (2FA
  enrolment) — plus old 13.1 — are replaced by **§19**, one thorough serial
  test that runs LAST: wizard → Create Account → assert (1 POST, no auth
  headers, `Previous` disabled, session stored, 2FA screen) → **enrol 2FA with
  a real computed TOTP** (wrong code → inline error; right code → enabled +
  backup codes) → Finish → back to `/login`.
- New **§18 i18n** (FR/DE/ES rendering + no raw `setup.*` keys).
- 4 tests that were previously masked by the no-op problem and failed once they
  actually ran were fixed (test-logic, not product bugs): 11.2 (check dup IDs on
  the step that has inputs), 12.2 (exclude the inlined icon sprite from the DOM
  budget), 16.2 / 17.1 (fill valid data so `Next` stays enabled during the
  navigation loops).
- A cookie-consent `beforeEach` (as in `tests/e2e/login`) keeps the bottom banner
  off the wizard footer buttons.

**A fresh server is required per run** — §19 really creates the admin account.

---

## Backend — Unit Tests (`src/setup.rs`)

### validate_username
- [x] `test_validate_username_empty` — empty string rejected
- [x] `test_validate_username_too_short` — < 3 chars rejected
- [x] `test_validate_username_too_long` — > 64 chars rejected
- [x] `test_validate_username_exact_min_length` — exactly 3 chars accepted
- [x] `test_validate_username_exact_max_length` — exactly 64 chars accepted
- [x] `test_validate_username_invalid_chars_space` — space rejected
- [x] `test_validate_username_invalid_chars_hyphen` — leading hyphen rejected
- [x] `test_validate_username_invalid_chars_at` — @ sign rejected
- [x] `test_validate_username_starts_with_digit` — digit start rejected
- [x] `test_validate_username_starts_with_hyphen` — hyphen start rejected
- [x] `test_validate_username_single_char` — single char rejected
- [x] `test_validate_username_reserved_root` — "root" rejected
- [x] `test_validate_username_reserved_admin` — "admin" rejected
- [x] `test_validate_username_reserved_case_insensitive` — "ROOT" / "Admin" rejected
- [x] `test_validate_username_reserved_all_listed` — all 26 reserved names checked
- [x] `test_validate_username_valid_simple` — "alice" accepted
- [x] `test_validate_username_valid_mixed_case` — "AliceAdmin" accepted
- [x] `test_validate_username_valid_with_numbers` — "user123" accepted
- [x] `test_validate_username_valid_with_underscore` — "user_name" accepted
- [x] `test_validate_username_valid_starts_with_underscore` — "_admin" accepted

### validate_password
- [x] `test_validate_password_empty` — empty rejected
- [x] `test_validate_password_too_short` — < 8 chars rejected
- [x] `test_validate_password_exactly_8_chars` — exactly 8 accepted
- [x] `test_validate_password_too_long` — > 128 chars rejected
- [x] `test_validate_password_exactly_128_chars` — exactly 128 accepted
- [x] `test_validate_password_exactly_129_chars` — 129 chars rejected
- [x] `test_validate_password_no_uppercase` — no uppercase rejected
- [x] `test_validate_password_no_lowercase` — no lowercase rejected
- [x] `test_validate_password_no_digit` — no digit rejected
- [x] `test_validate_password_valid` — "SecurePass1" accepted
- [x] `test_validate_password_valid_with_special` — "P@ssw0rd!" accepted

### validate_fullname
- [x] `test_validate_fullname_empty` — empty rejected
- [x] `test_validate_fullname_too_long` — > 128 chars rejected
- [x] `test_validate_fullname_exact_max` — exactly 128 accepted
- [x] `test_validate_fullname_valid` — "John Doe" accepted

### is_valid_username
- [x] `test_is_valid_username_valid` — "firstadmin" returns true
- [x] `test_is_valid_username_invalid` — "" returns false
- [x] `test_is_valid_username_reserved` — "root" returns false

### RESERVED_USERNAMES
- [x] `test_reserved_usernames_not_empty` — list is non-empty
- [x] `test_reserved_usernames_count` — exactly 26 entries

### generate_system_hash
- [x] `test_generate_system_hash_non_empty` — output is non-empty
- [x] `test_generate_system_hash_hex_format` — all chars are hex digits
- [x] `test_generate_system_hash_deterministic` — same input → same output

---

## Backend — Integration Tests (`tests/setup_integration.rs`)

### GET /api/auth/setup-status
- [x] `test_setup_status_first_run` — returns is_first_run=true when no admin exists
- [x] `test_setup_status_has_sudo_available_field` — sudo_available field present
- [x] `test_setup_status_has_machine_id` — server_machine_id non-empty

### GET /api/auth/check-username
- [x] `test_check_username_available` — valid username returns available=true
- [x] `test_check_username_reserved_root` — "root" returns available=false
- [x] `test_check_username_reserved_admin` — "admin" returns available=false
- [x] `test_check_username_too_short` — "ab" returns available=false
- [x] `test_check_username_invalid_chars` — "user+name" returns available=false
- [x] `test_check_username_missing_param` — no query param returns 4xx
- [x] `test_check_username_taken_after_setup` — username taken after creation
- [x] `test_check_username_different_one_still_available` — other names still available

### POST /api/auth/setup — validation
- [x] `test_setup_empty_username` — empty username returns error
- [x] `test_setup_reserved_username` — "root" returns reserved error
- [x] `test_setup_weak_password_no_uppercase` — missing uppercase returns error
- [x] `test_setup_weak_password_no_digit` — missing digit returns error
- [x] `test_setup_password_too_short` — < 8 chars returns length error
- [x] `test_setup_long_username` — > 64 chars returns length error
- [x] `test_setup_invalid_chars_username` — space/! returns char error
- [x] `test_setup_empty_body` — `{}` returns validation error
- [x] `test_setup_malformed_json` — non-JSON body returns 4xx

### POST /api/auth/setup — success flow
- [x] `test_setup_creates_admin_and_returns_session` — returns token + username + is_admin
- [x] `test_setup_marks_first_run_false` — is_first_run becomes false after setup
- [x] `test_setup_second_call_fails_already_complete` — second call returns error
- [x] `test_setup_with_fullname` — fullname is accepted
- [x] `test_setup_without_fullname` — setup works without fullname
- [x] `test_setup_with_avatar_seed` — custom avatar_seed is accepted

### Full flow
- [x] `test_full_setup_flow` — status → check-username → setup → check taken → verify not first run

### Concurrency
- [x] `test_concurrent_setup_only_one_succeeds` — two racing `POST /api/auth/setup`
  calls: exactly one succeeds (O_EXCL lock), the loser gets an
  "in progress / already complete" message, and only one user row is created

---

## Frontend — E2E Tests (`tests/e2e/setup/setup.spec.ts`)

### 1. Page load (6 tests)
- [x] 1.1 — wizard container is visible when server is in first-run state
- [x] 1.2 — login form is hidden when wizard is active
- [x] 1.3 — stepper has 4 steps with labels
- [x] 1.4 — step 0 (Welcome) is shown initially
- [x] 1.5 — first stepper dot is marked current
- [x] 1.6 — capabilities are loaded (no infinite spinner)

### 2. Welcome screen (6 tests)
- [x] 2.1 — welcome title is visible and non-empty
- [x] 2.2 — welcome messages are visible
- [x] 2.3 — three feature cards are shown
- [x] 2.4 — welcome icon is visible
- [x] 2.5 — Previous button is disabled on step 0
- [x] 2.6 — Next button is enabled on step 0

### 3. Step navigation (8 tests)
- [x] 3.1 — Next advances from step 0 to step 1
- [x] 3.2 — Previous goes back from step 1 to step 0
- [x] 3.3 — stepper updates when advancing
- [x] 3.4 — Previous button is disabled on step 0
- [x] 3.5 — Previous button is enabled on step 1
- [x] 3.6 — Previous button is disabled after returning to step 0
- [x] 3.7 — stepper line between done steps has done class
- [x] 3.8 — complete full flow: step 0 → 1 → 2 → 3

### 4. Account form (12 tests)
- [x] 4.1 — step 1 has title and description
- [x] 4.2 — username input: type=text, autocomplete=username
- [x] 4.3 — username has placeholder
- [x] 4.4 — fullname input: type=text, autocomplete=name, optional
- [x] 4.5 — avatar picker is visible
- [x] 4.6 — avatar preview or placeholder is visible
- [x] 4.7 — regenerate avatar button works
- [x] 4.8 — username validation: empty → button stays disabled
- [x] 4.9 — username validation: too short → button stays disabled
- [x] 4.10 — username validation: invalid chars → button stays disabled
- [x] 4.11 — username validation: reserved name → button stays disabled
- [x] 4.12 — valid username allows next step

### 5. Password form (12 tests)
- [x] 5.1 — step 2 has title and description
- [x] 5.2 — password input: type=password, autocomplete=new-password
- [x] 5.3 — confirm password input: type=password, autocomplete=new-password
- [x] 5.4 — password strength bar appears when typing
- [x] 5.5 — strength label shows correct level for weak password
- [x] 5.6 — strength label shows correct level for strong password
- [x] 5.7 — password requirement checklist shows all 4 items
- [x] 5.8 — requirement items show met/unmet state
- [x] 5.9 — mismatched passwords → button stays disabled
- [x] 5.10 — matching valid passwords allow next step
- [x] 5.11 — password validation: empty → button stays disabled
- [x] 5.12 — password validation: too short → button stays disabled

### 6. Verification step (7 tests)
- [x] 6.1 — step 3 has title and description
- [x] 6.2 — summary shows username
- [x] 6.3 — summary shows password as masked
- [x] 6.4 — verification auto-runs on step 3
- [x] 6.5 — verification shows success or error box
- [x] 6.6 — Create Account button is visible on step 3
- [x] 6.7 — summary shows fullname when provided

### 7 + 8. Account creation & 2FA enrolment → **moved to §19** (see below).
The old 10 tests here only ever passed as no-ops. Replaced by one real
end-to-end flow.

### 9. Prerequisites warning (2 tests)
- [x] 9.1 — sudo warning banner appears when sudo is unavailable
- [x] 9.2 — warning lists sudo as missing

### 10. Responsive (5 tests)
- [x] 10.x — renders correctly at 360px (phone)
- [x] 10.x — renders correctly at 390px (iPhone 14)
- [x] 10.x — renders correctly at 768px (tablet)
- [x] 10.x — renders correctly at 1024px (laptop)
- [x] 10.x — renders correctly at 1920px (desktop)

### 11. Accessibility (5 tests)
- [x] 11.1 — all visible inputs have associated labels
- [x] 11.2 — no duplicate IDs in the DOM
- [x] 11.3 — stepper dots have meaningful content (number or check)
- [x] 11.4 — buttons are keyboard-focusable (no tabindex=-1)
- [x] 11.5 — step content has proper heading hierarchy

### 12. Performance (5 tests)
- [x] 12.1 — wizard loads within 5 seconds
- [x] 12.2 — interactive DOM node count < 2000 (excludes the inlined icon sprite)
- [x] 12.3 — resource requests < 50
- [x] 12.4 — no memory leak across 5 navigations
- [x] 12.5 — first paint within 3 seconds

### 13. Security (6 tests)
- [x] 13.1 — setup POST carries no auth headers → **moved to §19** (it completes setup)
- [x] 13.2 — password fields are type=password (not visible)
- [x] 13.3 — password never appears in URL
- [x] 13.4 — XSS in username does not execute
- [x] 13.5 — no session token in localStorage before setup
- [x] 13.6 — CSRF token not stored before setup

### 14. Keyboard (4 tests)
- [x] 14.1 — Tab order on step 0: Next button is focusable
- [x] 14.2 — Enter on step 0 advances to step 1
- [x] 14.3 — Tab order on step 1: username → fullname → regenerate → next
- [x] 14.4 — Escape key does not navigate away

### 15. Console & network (3 tests)
- [x] 15.1 — no uncaught JS errors on wizard load
- [x] 15.2 — no console.error messages on clean wizard load
- [x] 15.3 — setup status check makes at most 3 API calls

### 16. Edge cases (5 tests)
- [x] 16.1 — very long username input does not break the page
- [x] 16.2 — rapid step navigation does not crash
- [x] 16.3 — fresh context shows wizard
- [x] 16.4 — navigating to unknown route shows login page
- [x] 16.5 — special characters in fullname do not break the page

### 17. Memory regression (3 tests, `test.slow()`)
- [x] 17.1 — step navigation ×10 (fills valid data so Next stays enabled) — no heap leak
- [x] 17.2 — username typing × 20 does not accumulate DOM nodes
- [x] 17.3 — avatar regenerate × 10 does not leak

### 18. i18n (4 tests)
- [x] 18.1 — French: welcome title, stepper label, Next button
- [x] 18.2 — German: Next button
- [x] 18.3 — Spanish: Next button
- [x] 18.4 — no raw `setup.*` keys leak into the wizard

### 19. Account creation & 2FA — RUNS LAST, completes first-run setup (1 test, `test.slow()`)
- [x] 19.1 — wizard → Create Account (1 no-auth POST, `Previous` locked, session
      stored, 2FA screen) → enrol 2FA with a **real computed TOTP** (wrong code →
      inline error; right code → enabled + backup-code list) → Finish → `/login`

---

## How to Run

```bash
# Backend unit tests (fast, no server needed)
cargo test setup::tests

# Backend integration tests (in-memory SQLite, no server needed)
cargo test --test setup_integration

# All backend tests
cargo test setup

# Frontend E2E tests (requires running server on :8080)
npx playwright test tests/e2e/setup/setup.spec.ts --project=firefox

# Single frontend test
npx playwright test tests/e2e/setup/setup.spec.ts -g "4.8"

# Custom server
BASE_URL=http://localhost:3000 npx playwright test tests/e2e/setup/setup.spec.ts
```
