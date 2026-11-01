# Security (sessions & login history)

**Purpose:** Let a user review/revoke their own sessions and review login history
(admin-only global history; per-user history is available to an admin or to the
account owner).

## Content Security Policy (2026-10-17)

`script-src` no longer allows `'unsafe-inline'`. The policy lists a **per-response
nonce** instead (`middleware::csp_nonce`):

- `security_headers_middleware` — the outermost layer — generates 16 random
  bytes (hex) per request and inserts them as `CspNonce` in the request
  extensions;
- `handlers::spa::serve_spa` replaces the `__HV_CSP_NONCE__` placeholder in
  `index.html` with that exact value, so the one inline boot script (the
  service-worker registration) carries `nonce="…"` matching the header;
- the two must agree **within one response** — a test asserts it.

Why it matters: with `'unsafe-inline'` and the session token in `localStorage`,
any injected script could read the token and exfiltrate a full session. A nonce
is unguessable, so injected markup cannot run.

`style-src` **keeps** `'unsafe-inline'`: Svelte emits scoped `<style>` blocks
and components set `style="…"` attributes, neither of which a nonce can cover.
Accepted debt — CSS cannot read the session token.

Adding an inline script later means adding `nonce="__HV_CSP_NONCE__"` to it in
`frontend/index.html`; anything without it will simply not run.

## Brute-force protection (2026-10-17)

There is deliberately **no fail2ban module**. An earlier
`src/middleware/fail2ban.rs` defined an in-memory IP ban list that no route or
handler ever referenced — dead code that read like an active protection. It was
removed; brute-force defence lives in three places instead:

- **Per account** — 5 failed logins inside 5 minutes lock the account, and the
  lock records an expiry (`users.locked_until`), so it lifts itself. See
  [auth.md](./auth.md) → *Login lockout and throttling*.
- **Per client IP, in memory** — 30 attempts/minute and 120/hour on
  `/auth/login` **and** `/auth/2fa/verify`, keyed by the TCP peer address. This
  resets on restart, which is why it is not the whole answer.
- **Per client IP, persisted** — the login auto-block below (QW-05): N failures
  inside a window create a `ip_bans` row that outlives a restart and is checked
  **before** a password is verified.

Do not reintroduce an unwired "protection" module: either wire it and test it,
or leave it out.

## Login auto-block and the request-rate row (2026-10-31, QW-05)

The missing piece between the two in-memory protections: a block that **survives
a restart** and can optionally **reach the firewall**. Settings → Security →
*Failed sign-in blocking* (`AutoBlockCard.svelte`, `data-testid="autoblock-*"`).

`src/auto_block.rs` (decisions), `src/handlers/auto_block.rs` (endpoints),
`src/storage/ip_ban_repo.rs` (`ip_bans` + `auto_block_settings`, migration
**065**), `src/middleware/auto_block.rs` (enforcement).

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /security/auto-block` | admin | policy, the ban list (active first) with the remaining minutes, `active`/`firewall_rules` counters, whether `ufw` is even available |
| `PUT /security/auto-block` | admin | save the policy — `{ enabled, threshold, window_seconds, ban_seconds, apply_firewall, dos_enabled, dos_requests_per_minute }`; validated only while enabled, then clamped |
| `POST /security/auto-block/pardon` | admin | `{ ip }` — lifts the row **and** its firewall rule; a block that is not there is a `400`, not a silent success |
| `POST /security/auto-block/clear` | admin | lifts every block, answering `{ lifted }` |
| `POST /security/auto-block/sweep` | admin | expires what is due now, answering `{ removed }` |

**Defaults: off**, 5 failures / 300 s → a 30-minute ban, no firewall rule, the
DoS row on at 600 requests/minute.

### The three invariants the acceptance criteria hang on

1. **Never the caller's own address, never loopback.** `must_not_ban` refuses an
   empty/non-address value, `127.0.0.0/8`, `::1`, `0.0.0.0`, `::`,
   `::ffff:127.0.0.1` — and the address the request itself came from, so an
   admin who mistypes their password five times does not lock themselves out of
   the box they are standing in front of. Loopback is also what the health
   checks, the local tools and any reverse proxy use.
2. **A ban is per address, never per account.** The legitimate user's address
   keeps its own clean counter, so a banned attacker cannot lock anyone out from
   elsewhere — the case the old per-account-only design could not answer.
3. **A pardon removes both halves.** The row records whether a `ufw` rule was
   installed (`firewall_rule`), so a pardon (and the expiry sweep) removes
   exactly the rule this feature added and never a hand-written
   `ufw deny from …`. The rule's comment is `homevault-autoblock-<ip>`.

### Enforcement, and its deliberate scope

- The **login handler** checks the ban **before** `authenticate`, so a blocked
  address cannot keep incrementing an account's failure counter, and records one
  more failure after each rejected attempt (`record_failure`, which does nothing
  while the feature is off or for a protected address).
- `middleware::auto_block` applies the same check to **unauthenticated `/api`
  calls only**. A session granted before the ban is **not** revoked here —
  "revoke sessions" is its own action with its own consequence — and a layer
  that can lock an authenticated admin out of the appliance is the foot-gun
  invariant 1 exists to prevent.
- The public document routes (`/share/*`, the ACME challenge) are outside that
  router, so a banned address can still fetch a share link it was given: a link
  is a separate credential and the ban is about *signing in*.
- A banned caller gets `403` with "This address is temporarily blocked after
  repeated failed sign-ins. Try again in about N minute(s), or ask an
  administrator to lift the block." and a `Retry-After` header.

### Expiry is self-healing

`sweep_expired` runs on the **read path** of every enforcement check (and from
the sweep endpoint), deleting each row whose `expires_at` has passed and
removing its firewall rule. That is what makes an expired ban lift itself on a
box nobody reboots; a scheduler for something this cheap would add a moving part
and could leave a ban alive past its expiry when a tick is missed. A row that is
expired but not yet swept is still shown on the card, labelled *block expired* —
it is honest about the row it still has.

### The request-rate row ("DoS")

An in-memory fixed-window counter (`middleware::rate_limit::RateLimitState`,
rebuilt whenever the configured rate changes) keyed by client address, applied to
**unauthenticated** `/api` calls: a flood is refused with `429` instead of
reaching the database. `0` means off, as does the switch. It deliberately does
**not** count signed-in traffic: a family behind one router shares one budget,
and throttling the appliance's own dashboard polling before it stops an attacker
would be a regression.

## Connected clients, and disconnecting one (2026-10-31, QW-06)

"Active Sessions" answers *who is signed in to the web UI*. This answers the
other, more frequent question — **why is the disk busy, and who is holding the
file?** — across SMB, NFS, SFTP and FTP. Settings → Security → *Connected
clients* (`ConnectedClientsCard.svelte`, `data-testid="clients-*"`).

`src/connected_clients.rs` (parsers + collector + kick),
`src/handlers/connected_clients.rs` (endpoints).

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /security/connected-clients` | admin | `{ clients[], sources[], kickable, checked_at }` — **always 200**; each source says `available`, `count` and, when not available, the tool's own `error` |
| `POST /security/connected-clients/kick` | admin | `{ id }` — disconnects one client. The body names a **client id, never a pid** |

### The three sources, and why each is read the way it is

| Source | Command | Rows |
|--------|---------|------|
| `smb` | `smbstatus -j` | one row per session: account, client address/host, the share(s) from `tcons`, the open-file **count** from `open_files` |
| `nfs` | `showmount -a` | one row per mount: client address and export path |
| `sessions` | `ss -H -tnp state established` | one row per inbound stream on 445/139/22/21, with the session process and its pid |

Three rules make the list trustworthy:

1. **A failing source never fails the list.** A box without Samba reports SMB as
   unavailable (with `smbstatus`'s own reason) and still shows its NFS mounts.
   Every command is bounded (`run_timed`, 5 s) and runs on the blocking pool, so
   a wedged `smbstatus` cannot stall the request.
2. **No paths.** `smbstatus` reports the absolute path of every open file; the
   row carries only the share name and the *count*. The card answers "who", not
   "what are they reading" — a unit test asserts that no `/srv/...` path in the
   sample reaches the payload.
3. **A kick re-collects first.** The request names a client id; `kick` runs the
   collection again and takes the pid **from that fresh observation**, so a
   stale or forged id cannot signal an unrelated process. That is the entire
   safety argument: a kick endpoint that trusted a client-supplied pid would be
   a remote signal primitive.

### What can be disconnected, and what cannot

- **SMB** — `smbcontrol <pid> close-share <share>`: drops the share connection
  and keeps the session (the user can reconnect). `smbcontrol kill` would drop
  every share of that user, which is a different action nobody asked for.
- **SFTP / FTP** — `kill -TERM <pid>` of *that connection's* process, taken from
  the fresh collection; the server's own pid is refused explicitly.
- **NFS** — **not kickable**, and the row says so instead of offering a button
  that fails: the export is kernel-held. The suggestion is to change the export,
  or to block the address in *Failed sign-in blocking*.

A refusal is specific: an unknown id answers "No client '…' is connected right
now — refresh the list and try again"; an NFS row answers with the kernel
explanation. Both are `400` with the sentence in `message`.

### The screen

The card lists one row per connection — protocol tag, address (and host name
when the source reports one), account, share/export, open-file count, and the
process evidence — plus a per-source status line so an empty section explains
itself. Disconnecting takes **two clicks** (the first turns the button into
"Disconnect now?"), and the confirmation is carried by text *and* an outline,
never colour alone. A row that cannot be disconnected shows that as a label.

## Backend

Handlers in `src/handlers/security.rs`.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /security/sessions` | auth | current user's sessions, each identified by an **opaque id** (`SessionInfo.id`) — the response never carries a live session token |
| `POST /security/sessions/revoke` | auth | revoke sessions by opaque id (`{ ids: [] }`); ids resolve **only** against the caller's own sessions, so a foreign id/token is a 404 and deletes nothing |
| `POST /security/sessions/revoke-all` | auth | revoke every session for the caller except the current one (caller-scoped by construction) |
| `GET /security/login-history` | admin | global login history — each row is **truthful**: the real `success` flag (failures stay, they are security-relevant) and the HTTP `User-Agent`; per-user failure counters only count `success = 0`, `clear_attempts` never purges successful logins |
| `GET /security/login-history/{username}` | admin or self | per-user history (same truthful success/user-agent per row); a non-admin may only read their own |
| `GET /security/active-sessions-count` | admin | distinct users with a currently valid session, system-wide (not the caller's own sessions) — powers the desktop "Active Sessions" monitor widget |
| `/security/auto-block*` | admin | the login auto-block — see the section above |

### Session identifiers (2026-10-22)

`session_public_id(token)` (`src/handlers/security.rs`) is a 12-character
SHA-256 fingerprint of the session token. It is what `GET /security/sessions`
returns and what `POST /security/sessions/revoke` accepts:

- **Never the token.** The old response mapped `token: s.token.clone()`, which
  put a replayable bearer token into the DOM, devtools, any client-side log and
  any XSS on the account page. The admin view already masked its tokens
  (`handlers/users.rs::mask_session_token`); the self-service view now returns
  no token at all.
- **Ownership is enforced at resolution, not at deletion.** The revoke handler
  loads `find_all_by_username(caller)` and keeps only the sessions whose public
  id matches the request. A token/id belonging to another account is not in the
  candidate set, so it cannot be revoked even if the attacker knows it
  (CWE-639). A request that resolves nothing answers **404** ("No matching
  session to revoke for your account"), an empty `ids` array answers **400**.
- **The current session stays protected.** Its own id never resolves (it is
  filtered before deletion); "log out everything else" is the dedicated
  `revoke-all` endpoint.

### One auth primitive, one status contract (2026-10-24, ROADMAP X.7 / X.8)

Every handler in `security.rs` used to parse `x-session-token` itself and rebuild
the same three branches (header present → resolve → is-admin). Six copies of the
same decision is how the answer drifts, and it had:

- `security.rs`, `shares.rs`, `permissions.rs`, `snmp.rs` and `settings.rs`
  answered **`400 Bad Request`** to a signed-in **non-admin** where every other
  module answered `403`;
- a **missing** `x-session-token` answered **`400`** across the API (the header
  was "just another absent parameter") while `terminal_pty.rs`, `system.rs` and
  a few others answered `401`.

A client that branches on the status therefore read "you are not signed in" as
"your request is malformed", and the same refusal looked like two different
problems on two screens. Both are fixed at the root:

- `handlers::admin_guard` is now **the only module that turns the header into an
  identity decision**. The eight modules that carried a private
  `require_admin` (`syncthing`, `shares`, `vpn`, `ups`, `permissions`, `snmp`,
  `user_homes`, `downloader`), the `Explorer`/`Settings`/`Chat`/`Avatars` inline
  blocks and `handlers::auth::require_admin` (a duplicate of the shared one) all
  delegate to it. Modules that answer a `Json<ApiResponse>` envelope keep their
  envelope and flatten the guard's *status* to a message via
  `admin_guard::denial_message`.
- The status is stated once, in `admin_guard.rs` and in
  [API.md](../API.md) → *Auth status contract*: `401` for an absent/empty header
  or an unknown/expired token, `403` for a valid non-admin on an `admin` route,
  and `400` **only** for a request the server could not understand.
- Source guards in `admin_guard.rs` fail the build if a module defines its own
  `require_admin`/`require_auth` again, or if one of the eight delegated modules
  starts parsing the header itself. Behaviour guard:
  `auth_guard_integration.rs` (missing → `401`, non-admin → `403`, malformed →
  `400`, on the same routes).

## Frontend (auto-block card)

`frontend/src/lib/components/settings/AutoBlockCard.svelte`, in
**Settings → Security**, between *Reachability* and *Antivirus*. Fields: the
enable switch, threshold, window, ban duration (with the humanised duration
beside it), the firewall switch (disabled wording when `ufw` is absent), the DoS
switch and rate, then the block list — one row per address with its remaining
minutes, its reason, whether a firewall rule is installed, and a **Pardon**
button. The answer of a refused save is the server's own sentence, verbatim,
because it names the field that cannot work.

`api.ts`: `getAutoBlock`, `setAutoBlock`, `pardonAutoBlock(ip)`,
`clearAutoBlockBans`. i18n keys `settings.autoblock_*` ×4 locales at parity.

## Frontend

- Security UI: the Settings security-category cards
  (`frontend/src/lib/components/settings/TlsCard.svelte`,
  `FirewallCard.svelte`, `AntivirusCard.svelte`, `AntiRansomwareCard.svelte`)
  plus the login-history view in `frontend/src/routes/Account.svelte`.
- `frontend/src/lib/services/api.ts`: `getMySessions`, `revokeSessions`,
  `revokeAllSessions`, `getLoginHistory`, `getUserLoginHistory`,
  `getActiveSessionsCount`.
- The "Active Sessions" desktop monitor widget (`frontend/src/routes/
  Desktop.svelte`, `.hv-sessions-monitor`) is a separate consumer of
  `getActiveSessionsCount`, owned by the Widgets app (`docs/guides/widgets.md`).

## Invariants (auto-block)

- **A ban never blocks the address that is configuring the box**, never
  loopback, never an unparsable "unknown": `auto_block::must_not_ban` is the
  single place that decides, and it is unit-tested.
- **The row is the authority, not the firewall.** A ban is stored even when the
  `ufw` rule fails (no ufw, no root), and `firewall_rule` records what was
  actually installed — a pardon trusts that flag, never the policy.
- **Bans are keyed by address** (one row per IP, `UNIQUE`), and re-banning
  extends the existing row instead of stacking duplicates.
- **An expired ban does not block**, even before the sweep deletes it: the
  lookup compares against `now`, so a clock skew or a missed sweep cannot
  refuse a legitimate sign-in.
- **Fail open, loudly.** A database error while reading bans logs an error and
  allows the request: a ban list must never be able to lock the appliance.
- **`ufw` is never called from a request thread without a deadline** — every
  invocation goes through `run_ufw`/`run_ufw_checked` (30 s, `UFW_TIMEOUT`).
- The `ufw` comment is `homevault-autoblock-<ip>` and is validated with
  `validate_comment` (no quotes, no control characters) so it survives ufw's
  single-quoted round-trip; `unban_source` tolerates "rule does not exist".

## Invariants

- Revoking a session invalidates that `X-Session-Token` server-side immediately.
  `revoke-all` keeps the caller's **current** session alive — it signs out the
  *other* devices only, so the current client does not need to log in again.
- **No live session token is ever returned by the self-service API** — only the
  opaque id from `session_public_id`. Guard:
  `account_integration.rs::test_my_sessions_never_returns_a_live_session_token`.
- **Revocation is caller-scoped.** `POST /security/sessions/revoke` resolves the
  submitted ids against the caller's own sessions only; a foreign token/id is a
  404 and leaves the other account signed in. Guard:
  `account_integration.rs::test_revoke_sessions_is_scoped_to_the_caller`.
- Login history: the global view is admin-only; per-user history is admin **or**
  the account owner (`handle_get_user_login_history`).
- **Every handler in this module guards through `admin_guard`** — no local copy
  of the token check, no local status choice. Source guard:
  `admin_guard.rs::security_module_never_parses_the_header_itself`.
- **An expired session is `401` on all three self-service endpoints** and writes
  nothing. Guard:
  `account_integration.rs::test_security_endpoints_reject_an_expired_session`.
- `getActiveSessionsCount` counts **distinct usernames** with a
  non-expired session (`SessionRepository::count_distinct_active_users`),
  not raw session rows — one user open in several tabs/devices still
  counts once. Don't reuse `SessionRepository::count()` for this: it
  counts every row including already-expired ones not yet swept by
  `delete_expired`, and doesn't dedupe by user.

## Tests for connected clients

- `src/connected_clients.rs` — **16 unit**: `smbstatus -j` sessions become one
  row each with their share(s) and open-file count (string *and* numeric pids);
  an SMB session without a share is not kickable; **no file path reaches the
  row** (asserted on the serialised JSON); non-JSON `smbstatus` is an explicit
  error while `{}` is an empty list; `showmount -a` lines become never-kickable
  NFS rows and junk lines are skipped; `ss -tnp` streams are filtered to the
  managed ports and to `ESTAB`, IPv6 and header lines are handled, a stream
  without a process is visible but not kickable, and several streams of one
  session are one row; the merge keeps SMB's account/share *and* the stream's
  pid, and leaves unrelated rows (including the same address on another
  protocol) alone; the two refusal messages.
- `tests/connected_clients_integration.rs` — **6**: the admin matrix on both
  routes; **the list always answers `200` with three sources, each carrying
  `available`/`count` and, when unavailable, a non-empty `error`**; the envelope
  (`clients`, `kickable`, `checked_at`); no real path is ever in the payload; an
  unknown client id is a `400` naming "is connected right now … refresh"; an
  empty id is a `400`; and the NFS refusal sentence.
- `tests/e2e/settings/connected-clients.spec.ts` — **9 `test(`**: the card with
  one status per source; an unavailable source naming the tool's reason; the
  list showing address/account/share/open-count and a Disconnect button for the
  SMB row while the NFS row offers none; **disconnecting taking two clicks**
  (nothing is sent on the first) and reporting the server's detail; a refused
  disconnect as `role="alert"`; a member refused on read and kick; the
  disconnect button's accessible name naming the client; no duplicate ids; no
  horizontal overflow. The list and the kick are **mocked** in this file.

## Tests for the auto-block

- `src/auto_block.rs` — **16 unit**: the threshold is inclusive (`>=`), a
  disabled policy never bans, the ban window's timestamps, the protected-address
  matrix (loopback v4/v6, `::ffff:127.0.0.1`, unspecified, empty, `unknown`,
  `1.2.3.1/24`), "the caller never bans its own address", the defaults, the
  field-by-field validation of an enabled policy (and that a disabled one may
  keep any numbers), `clamped`, `is_active` at the exact expiry, remaining
  seconds/minutes (never negative), the blocked wording, the DoS limiter
  (`None` when off or `0`), and that a ban comment cannot break out of its
  quotes.
- `src/firewall.rs` — **2 new unit**: an address is validated before it becomes
  a `ufw` argument (host names and shell fragments refused) and the auto-block
  comment passes `validate_comment` while one with a quote does not.
- `tests/auto_block_integration.rs` — **14**: the admin matrix on all five
  routes; the shipped defaults on a fresh box; a policy round-trip; six invalid
  policies refused with nothing stored; a disabled policy allowed to keep
  impossible numbers; **repeated failures banning the address** and the address
  then being refused even with the right password (message + `Retry-After`
  wording); the ban **surviving a restart** (a fresh router over the same pool);
  **a ban never locking out another address**; loopback and non-addresses never
  banned (and the admin still able to sign in); nothing banned while the policy
  is off; a pardon lifting the block immediately (and a pardon of an unknown
  address being a `400`); `clear` lifting three rows; an **expired ban lifting
  itself** through the login path with the row swept; the ban surviving a failed
  firewall rule with `firewall_rule` following `ufw_available()`; and the DoS
  row round-tripping and never storing above its ceiling.
- `tests/e2e/settings/autoblock.spec.ts` — **14 `test(`**: the card renders its
  policy fields, the block list and the empty state; the defaults; a save
  round-trip; an impossible policy refused with the reason on screen; a
  disabled save accepted; the firewall switch explained when `ufw` is absent; the
  DoS field following its switch; a pardon of a seeded row (mocked, so no `ufw`
  is touched); "clear every block"; a member refused on read/write/pardon;
  labels and accessible names; no duplicate ids; no horizontal overflow.

**No test invokes `ufw`.** The firewall half is asserted as *generation* (argv
and comment validation) plus the recorded flag; the decision logic is pure. The
live half — a real rule appearing in `ufw status` — needs root and is documented
as a manual check in the guide's *Live verification* note.

## Related

- [Authentication](./auth.md), [Settings](./settings.md)
