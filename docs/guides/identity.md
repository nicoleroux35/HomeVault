# Identity: trusted devices and rescue e-mail

**Purpose:** make a second factor survivable. Two things stop "I have to type a
code every day" from turning into "I turned 2FA off": a browser can be trusted
for a bounded time, and an account can name a verified address that can receive a
recovery code when the password is gone. Lives in **My Account** (trusted devices,
rescue address) and on the sign-in screen (recovery).

## Overview

`src/identity.rs` holds the storage and the security-relevant pure helpers;
`src/handlers/identity.rs` exposes the endpoints; `src/handlers/auth.rs` consults
the trusted-device cookie on the sign-in path.

Two rules shape everything:

- **Nothing sensitive is stored in the clear.** A trusted-device cookie is stored
  as a SHA-256 hash, and a rescue code as a salted hash with an attempt counter. A
  copy of the database must not be enough to skip a second factor or take over an
  account.
- **A trusted device is a time-boxed convenience, not a credential.** It expires,
  the owner can list and revoke it, and every sign-in through it is still recorded
  in the login history.

## Trusted devices

| Property | Value |
|---|---|
| Cookie | `hv_trusted_device`, `HttpOnly`, `SameSite=Strict`, `Secure` over TLS |
| Lifetime | 30 days (`DEVICE_LIFETIME_DAYS`) |
| Stored form | SHA-256 of the token, hex |
| Label | derived from the user agent — "Firefox on Linux", "Safari on iOS" |

The label is deliberately coarse: a home appliance is reached from a handful of
clients, and the owner needs to recognise one in a list, not identify a build.

**Sign-in path.** A correct password on an account with 2FA asks for a second
factor — unless the request carries a device cookie whose row belongs to *that
account*. A device of somebody else is ignored: the cookie is not a credential for
any account but its own. The device is touched on use, and an expired row is
deleted on sight rather than left to accumulate.

**Disabling 2FA revokes every device**, so re-enabling it does not silently
restore trust that was granted for an older configuration.

The raw token exists only in the `Set-Cookie` header: the database never holds it.

## Rescue e-mail

An address is stored on the account (`users.rescue_email`) only once a code sent
to it has come back. Saving it optimistically would let a typo hand account
recovery to a stranger.

| Property | Value |
|---|---|
| Code | six digits, uniform over `000000`–`999999` |
| TTL | 15 minutes (`CODE_TTL_MINUTES`) |
| Stored form | SHA-256 salted with the account name **and** the address |
| Attempts | 5 wrong guesses destroy the code (`MAX_CODE_ATTEMPTS`) |
| Reuse | a correct code is deleted as it is consumed |

One outstanding code per account. `purpose` separates "prove you own this
address" from "recover this account", so a code issued for one cannot be replayed
against the other.

## Account recovery

A locked-out user has no session, so the two recovery routes are public. Both
answer identically whether or not the account exists, has a rescue address, or was
throttled — this endpoint must not tell an attacker which names are real.

1. `POST /api/auth/recover { username }` — sends a code to the **verified**
   address, if there is one. Throttled to one mail per account per minute, keyed
   on the submitted name so a wrong guess and a right one take the same time.
2. `POST /api/auth/recover/verify { username, code }` — a correct code proves
   control of the mailbox and is exchanged for a **fresh offline recovery code**.
   The client is then sent to the ordinary reset screen with that code.

Exchanging for the existing offline recovery code means there is one
password-reset path, with one password policy and one attempt limit, instead of a
second weaker one.

## Passkeys (WebAuthn, Phase 1 P1.3)

A passkey lets an account sign in with the device's own authenticator — Touch
ID, Windows Hello, an Android/iOS passkey, a hardware security key — instead of
a password. It is a **separate credential**: the password and the TOTP factor
keep working exactly as before, and revoking a passkey never removes either.

### The ceremonies

Both are two calls: `begin` (the server generates a single-use challenge) then
`finish` (the server verifies the signed response). Everything cryptographic is
verified **server-side**, in `src/passkey.rs`, which is a small self-contained
WebAuthn implementation — a CBOR reader for the attestation object and the COSE
key, an `authenticatorData` parser, and `ring` for the ES256 / EdDSA signature.
No third-party WebAuthn crate is involved, so every rule the spec makes
security-relevant is a unit test in this tree.

| Check | Rule |
|---|---|
| `clientData.type` | `webauthn.create` on registration, `webauthn.get` on sign-in — a registration response can never authorise a sign-in |
| `clientData.challenge` | equal (constant-time) to the challenge **this** ceremony issued; consumed on first use |
| `clientData.origin` | exact equality with the configured origin — `https://vault.home.evil.example` is not accepted for `https://vault.home` |
| `rpIdHash` | `SHA-256(rp id)` |
| `UP` flag | required on both ceremonies |
| `UV` flag | **preferred, not required** — a hardware key without a PIN must still work; a credential that reported UV is marked as such in the list |
| COSE key | ES256 (P-256) or EdDSA (Ed25519) only; anything else is an explicit refusal |
| signature | over `authenticatorData ‖ SHA-256(clientDataJSON)` |
| `sign_count` | must increase when the authenticator reports one; a regression is refused (a cloned authenticator). `0` means "no counter" and is exempt, per the spec |

**Attestation is not verified, and the docs say so.** `fmt = "none"` and
self-attestation are accepted — that is what platform authenticators ship — and
the format is recorded as evidence. HomeVault makes no claim about the
authenticator's provenance, and a future attestation policy can read the stored
format without a migration.

### The relying party

`rp.id` is the hostname and `rp.origin` the exact scheme + host + port. Both are
derived from **configuration**, never from the request's `Host`/`Origin` header,
so a caller cannot widen the origin by sending a different one:

1. `HOMEVAULT_HOSTNAME` (what the installer and the tests set), else
2. the ACME domain configured for a public name, else
3. the machine's own host name.

`rp.id` is always the lowercased hostname with no port. `rp.origin` includes the
port whenever it is not the scheme's default — HomeVault's HTTPS listener is
**8443**, not 443, so a deployment that terminates TLS in front of it on 443 must
set `HTTPS_PORT=443` (or name the port in the configured hostname) or every
ceremony will be refused as a foreign site. The Account card shows the origin it
is bound to, precisely so this is visible instead of puzzling.

### Endpoints

| Method | Path | Guard |
|---|---|---|
| `GET` | `/api/passkeys` | signed in |
| `POST` | `/api/passkeys/register/begin` | signed in |
| `POST` | `/api/passkeys/register/finish` | signed in |
| `POST` | `/api/passkeys/login/begin` | public |
| `POST` | `/api/passkeys/login/finish` | public |
| `PATCH` | `/api/passkeys/{id}` | signed in (own credential) |
| `DELETE` | `/api/passkeys/{id}` | signed in (own credential) |
| `POST` | `/api/passkeys/sweep` | admin |

A rename or a revoke of another account's credential is a **`404`**, not a
`403`: the id must not be confirmed to exist.

### Signing in

`login/begin` accepts an optional `username`:

- **with one**, `allowCredentials` names that account's passkeys;
- **without one**, a discoverable credential picks the account on the
  authenticator (the challenge lives in an anonymous namespace, and the account
  is resolved from the credential id at `finish`).

An unknown username gets the **same answer shape** as a known one (a real
challenge, an empty `allowCredentials`) — no enumeration oracle on the sign-in
screen. A refused assertion answers a single sentence ("Passkey sign-in failed")
that never says whether the account exists.

On success the server creates a **normal session**, records the successful
attempt in the login history (so the passkey is visible where every other sign-in
is), and applies the same account-state rules as the password path: a locked
account cannot sign in with a passkey, or the passkey would be a way around an
admin's lock.

### Schema

`sqlx/migrations/067_passkeys.sql`:

- `passkey_credentials(credential_id, username, label, algorithm, public_key,
  sign_count, attestation_fmt, aaguid, user_verified, created_at,
  last_used_at)` — a **public** key only, cascade-deleted with the account; at
  most 20 per account;
- `passkey_challenges(username, ceremony, challenge, created_at, expires_at)` —
  one outstanding challenge per (account, ceremony), `RETURNING`-deleted so it is
  single use by construction, and swept when it expires (5 minutes).

### Frontend

- `stores/passkey.ts` — the browser half: options → `navigator.credentials`,
  credential → base64url payload. Every conversion lives there, because one
  encoding slip makes a signature fail with no clue.
- `components/account/PasskeysCard.svelte` — the list, the rename and revoke
  actions, the "add a passkey" form and the origin the passkeys are bound to.
- `routes/Login.svelte` — the "Sign in with a passkey" button, shown only when
  the browser can run a ceremony, and only reachable once a username is typed
  (that is whose passkeys to offer).

A cancelled prompt and an unsupported browser are reported as **what they are**,
never as a wrong password.

### Tests

- Unit (`src/passkey.rs`, **42**): every check above in both directions, the
  origin matrix (lookalike hosts, lookalike schemes, a trailing slash), the CBOR
  and COSE refusals (unsupported algorithm, wrong curve, short coordinate,
  trailing bytes), the attestation object's missing fields, the relying party's
  derivation, and **real crypto** — a generated P-256 key pair signs a real
  assertion, and a tampered payload, a foreign key and a counter regression are
  all refused.
- Integration (`tests/passkey_integration.rs`, **16**): a software authenticator
  built in the test registers through the real endpoints, then signs in through
  them, producing a session; the replay of a consumed challenge, a mismatched
  credential id, a foreign account's rename/revoke, a bad signature, an unknown
  credential, a locked account, an expired challenge and the no-enumeration shape
  of `login/begin`.
- E2E (`tests/e2e/account/account.spec.ts` §18, `tests/e2e/login/login.spec.ts`
  §27): the card's states and actions against a mocked endpoint, and the login
  button's contract — including the "cancelled is not a bad password" case. The
  crypto is not re-tested through a browser, which has no authenticator.

## Endpoints

| Method | Path | Guard |
|---|---|---|
| `GET` | `/api/identity/devices` | signed in |
| `POST` | `/api/identity/devices/revoke` | signed in |
| `POST` | `/api/identity/devices/revoke-all` | signed in |
| `GET` | `/api/identity/rescue-email` | signed in |
| `POST` | `/api/identity/rescue-email` | signed in |
| `POST` | `/api/identity/rescue-email/verify` | signed in |
| `DELETE` | `/api/identity/rescue-email` | signed in |
| `POST` | `/api/auth/recover` | public |
| `POST` | `/api/auth/recover/verify` | public |

Every signed route acts on the caller's own account: the username comes from the
session, never from the body, so there is no id to tamper with. Revoking a device
that belongs to another account is a miss, not a cross-account delete.

## Schema

`sqlx/migrations/060_identity.sql`:

- `trusted_devices(id, username, label, token_hash, created_at, last_seen_at,
  expires_at)` — one row per browser, cascade-deleted with the account;
- `rescue_email_codes(username, email, purpose, code_hash, attempts, created_at,
  expires_at)` — one outstanding code per account;
- `users.rescue_email`, `users.rescue_email_verified_at` — the verified address.

Both tables are additive: an existing account has no trusted device and no rescue
address, which is exactly the old behaviour.

## Mail delivery

Codes go through the relay configured under **Notifications**
(`EMAIL_SETTINGS` → `EmailConfig` → `EmailService::send_email`), inside
`spawn_blocking` because sending shells out to an MTA. If delivery fails while
verifying an address, the pending code is dropped: a code nobody received must not
sit there waiting to be guessed.

## Tests

- Unit (`src/identity.rs`): token uniqueness and shape, hash stability and
  irreversibility, the six-digit code, the account-and-address salt, device labels
  across common clients, address validation, RFC 3339 expiry ordering, and the
  purpose round-trip.
- Unit (`src/handlers/identity.rs`): cookie parsing among other cookies, the
  `HttpOnly` / `SameSite=Strict` / 30-day attributes, no `Secure` over plain HTTP,
  the per-account mail throttle, and the wording of both code mails.
- Integration (`tests/identity_integration.rs`): 401 on every signed route;
  listing, revoking one device, revoking all; refusing to revoke another
  account's device; that an address is *not* stored until the code comes back; the
  right code storing it and a wrong one not; single use; clearing; a trusted
  device skipping the second factor while another account's does not; that a wrong
  password never reaches the device check; and that account recovery answers the
  same for an unknown account.
- E2E (`tests/e2e/account/identity.spec.ts`): the empty trusted-device state and
  the disabled "revoke every device" button, the rescue form, a malformed address
  refused without storing anything, an unknown account getting the generic
  recovery answer, a wrong code keeping the user signed out, and the opt-in
  switch producing an `HttpOnly` + `SameSite=Strict` cookie. The suite provisions
  throwaway accounts and sends no mail.

## Related

- [Authentication](./auth.md), [My Account](./account.md), [Security](./security.md),
  [Notifications](./notifications.md)
