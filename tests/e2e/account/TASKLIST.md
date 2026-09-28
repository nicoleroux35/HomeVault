# "My Account" page — Test Tasklist

> Screen: `frontend/src/routes/Account.svelte` (opened from the desktop
> top-bar user menu — it is not a route). Covered by the **"My Account"
> page — frozen** section of the root `CLAUDE.md`. These tests only add
> coverage; no production file was changed. If a test starts failing
> because of a real product change, stop and report it rather than
> loosening the assertion.

## Summary

| Layer | File | Tests | Status |
|-------|------|-------|--------|
| Frontend E2E | `tests/e2e/account/account.spec.ts` | 67 | ⏳ first run pending |

The backend endpoints the page uses (`GET /api/auth/me`,
`POST /api/auth/me/avatar`, `POST /api/auth/2fa/setup`,
`PUT /api/users/:name`, `PUT /api/users/:name/password`) are already
exercised by `tests/login_integration.rs` and the Users-page suites;
this file focuses on the account screen's own behaviour end-to-end.

---

## How to run

The account page lives inside the desktop shell, so the suite needs a
live server whose setup is complete and that has a seeded admin. It
provisions its own throwaway accounts through the admin API and deletes
them in `afterAll` — the seeded admin and its 2FA state are never
touched.

```bash
# 1. Start a scratch server on its own port + data dir
PORT=8199 DATA_DIRECTORY=/tmp/hv-e2e HOMEVAULT_SKIP_SUDO_CHECK=1 ./target/release/homevault &

# 2. Complete first-run setup as the seeded admin
curl -s -X POST http://localhost:8199/api/auth/setup \
  -H 'Content-Type: application/json' \
  -d '{"username":"e2eadmin","password":"E2eSecret123","fullname":"E2E Admin"}'

# 3. Run
BASE_URL=http://localhost:8199 npx playwright test tests/e2e/account/account.spec.ts --project=firefox

# Single test / category
npx playwright test tests/e2e/account/account.spec.ts -g "7.7"
npx playwright test tests/e2e/account/account.spec.ts -g "Security"

# Override the seeded admin
E2E_ADMIN_USER=e2eadmin E2E_ADMIN_PASS=E2eSecret123 \
  npx playwright test tests/e2e/account/account.spec.ts --project=firefox
```

The whole file runs `describe.configure({ mode: 'serial' })` — a shared
`beforeAll` creates one throwaway admin (`readOnly`) for the read-only
suites; each mutating suite owns its own account.

---

## Coverage (`account.spec.ts`)

### 1. Page load (5)
- [ ] 1.1 the account window opens from the user menu
- [ ] 1.2 header shows the "My Account" title + a subtitle
- [ ] 1.3 an admin account shows the "Administrator account" subtitle
- [ ] 1.4 the three sections render (Profile / Password / Two-factor)
- [ ] 1.5 no infinite spinner — the loading state clears

### 2. Identity card (5)
- [ ] 2.1 full name + `@username · email` line
- [ ] 2.2 "Member since" line with a real formatted date
- [ ] 2.3 an avatar image is rendered
- [ ] 2.4 the regenerate button is present and enabled
- [ ] 2.5 regenerate changes the avatar `src` and re-enables the button

### 3. Profile form — fields & attributes (5)
- [ ] 3.1 username field prefilled + disabled (read-only)
- [ ] 3.2 full-name field editable + prefilled
- [ ] 3.3 email field `type=email` + prefilled
- [ ] 3.4 every field has an associated `<label>`
- [ ] 3.5 "Save profile" button present

### 4. Profile update — submission (3)
- [ ] 4.1 edit name + email → save → persists across a reopen
- [ ] 4.2 the change is visible through `GET /api/auth/me` (real persistence)
- [ ] 4.3 the Save button does not stay stuck disabled

### 5. Password form — fields & validation (6)
- [ ] 5.1 three `type=password` inputs with `current-password` / `new-password` autocomplete
- [ ] 5.2 inputs start empty
- [ ] 5.3 empty current password → inline "required" error, no navigation
- [ ] 5.4 too-short new password → inline error
- [ ] 5.5 mismatched new/confirm → inline error
- [ ] 5.6 wrong current password (non-admin) → server error inline, stays on desktop
- Uses a dedicated **non-admin** account — the backend only verifies the
  current password for a non-admin self-change (`handle_user_update_password`
  exempts admins).

### 6. Password change logs out (1)
- [ ] 6.1 a valid change → redirect to `/login`, `session-token` cleared,
      old token dead server-side, new password authenticates

### 7. 2FA enrolment — disabled state (7)
- [ ] 7.1 fresh account shows "Disabled" badge + enable button
- [ ] 7.2 enable reveals a secret key + a numeric 6-digit `one-time-code` field
- [ ] 7.3 the setup link is an `otpauth://totp/…` URI
- [ ] 7.4 a bad code → error, 2FA stays disabled (checked over the API)
- [ ] 7.5 missing password → rejected before the request
- [ ] 7.6 Cancel returns to the toggle view without enabling
- [ ] 7.7 a correct TOTP code enables 2FA and reveals ≥ 8 backup codes;
      "Done" returns to the "Enabled" state

### 8. 2FA management — enabled state (5)
- [ ] 8.1 "Enabled" badge + "View backup codes" / "Disable" buttons
- [ ] 8.2 "View backup codes" lists the codes
- [ ] 8.3 disable with empty password → validation error, still enabled
- [ ] 8.4 disable with wrong password → server error surfaced
- [ ] 8.5 disable with the correct password → back to "Disabled" (verified over the API)
- `beforeAll` enrols 2FA over the API; tests self-skip if that fails.

### 9. Navigation (3)
- [ ] 9.1 unauthenticated `/desktop` → redirect to `/login`
- [ ] 9.2 the window closes from its title-bar close button
- [ ] 9.3 the window can be reopened after being closed

### 10. Responsive (3) — 360 / 768 / 1920 px
- [ ] no horizontal overflow; full-name field + Save button stay visible

### 11. Accessibility (4)
- [ ] 11.1 every visible input has a `<label for>`
- [ ] 11.2 the window introduces no duplicate DOM IDs
- [ ] 11.3 the header uses a single `<h1>`
- [ ] 11.4 no `tabindex="-1"` on interactive controls

### 12. Keyboard (2)
- [ ] 12.1 Tab from full-name lands on email
- [ ] 12.2 the 2FA code box enforces the 6-digit cap

### 13. Console & network (3)
- [ ] 13.1 no uncaught JS errors when opening the window
- [ ] 13.2 ≤ 1 `/auth/me` request on open (beyond the desktop's own auth check)
- [ ] 13.3 a profile save fires exactly one `PUT /users/:name`

### 14. Security (5)
- [ ] 14.1 no password value is ever written to `localStorage`
- [ ] 14.2 password fields never switch to `type=text`
- [ ] 14.3 the 2FA secret never lands in the URL
- [ ] 14.4 an XSS payload in the full-name field renders as text (no dialog, no markup)
- [ ] 14.5 `/auth/me` rejects a tampered session token

### 15. Edge cases (4)
- [ ] 15.1 a very long full name does not break the layout
- [ ] 15.2 a unicode full name round-trips through a save
- [ ] 15.3 rapid double-click on "Save profile" fires ≤ 1 write
- [ ] 15.4 opening the window with non-auth localStorage cleared still renders

### 16. Memory regression (`test.slow()`) (2)
- [ ] 16.1 open/close the window ×5 → ≤ 150 net DOM nodes
- [ ] 16.2 2FA enrol/cancel cycle ×5 → < 5 MB heap growth

### 17. i18n — the saved `hv-language` drives the account strings (4)
- [ ] 17.1 French — header / subtitle / section titles / save button
- [ ] 17.2 German — header + save button
- [ ] 17.3 Spanish — header + save button
- [ ] 17.4 no raw `account.*` keys leak into the rendered page

---

## Notes / gotchas

- **No `/account` route.** Every test injects a session into
  `localStorage` (`session-token` + `csrf-token`), loads `/desktop`, opens
  the top-bar user menu (`.hv-user-button`), and clicks the first
  `.hv-user-menu-item`.
- **Cookie banner** is pre-seeded via `addInitScript` (same trick as the
  login/setup suites) so it never overlays the desktop.
- **Admin self password-change is exempt from the current-password check**
  server-side (`handle_user_update_password`: `current_password` is forced
  to `None` when `session.is_admin`). The password-form suite therefore
  uses a non-admin account so §5.6 / §6.1 exercise a real verification.
- **TOTP window roll-over**: §7.7 retries once on a fresh 30 s window,
  like the login suite's 2FA test.
- **Throwaway accounts**: created via `POST /api/users` (admin session),
  deleted in `afterAll`. A killed run can leak `acct_*` users — start
  from a fresh data directory if that matters.

## Observations for the page owner (not fixed — frozen area)

- The 2FA enrol step shows the **secret key + an `otpauth://` link**, not a
  rendered QR image. `SCREENS.md` §4 ("QR code") overstates this.
- `profileSuccess` in `Account.svelte` is declared and cleared but never
  assigned — a successful profile save shows no explicit confirmation
  message (the identity card updating is the only feedback). The
  `account.password_changed_relogin` key is likewise never rendered.
