# Directory and single sign-on (LDAP / AD / OpenID Connect / SAML)

**Purpose:** let people sign in with the account they already have — on the
office directory server, or at the identity provider that already guards their
mail and their cloud accounts — instead of a second HomeVault password. An admin
picks **one** of the three providers on the screen, points HomeVault at it, says
which attribute or claim holds the login name and which groups mean
**Administrator**; from then on a member of those groups signs in there and lands
as an administrator here.

All three halves feed the same `remote_identities` table, the same group→role
resolution and the same break-glass rule, so an appliance can move from LDAP to
OpenID Connect or SAML (or run any of them) without a second account model.

The directory only ever **adds** a way in. A local account keeps its own
password, its 2FA and its ability to sign in while the directory is unreachable,
misconfigured or switched off — that is the break-glass rule this feature is
built around.

**Who can use it:** the screen and every **settings** endpoint are
**admin-only**. Signing in *through* a provider is of course open to anyone that
provider vouches for, so the four `/api/auth/*` routes a signed-out browser needs
are public by design (see [Endpoints](#endpoints)).

## Where it lives

- **Screen:** desktop app **Directory sign-in** → `frontend/src/routes/Directory.svelte`
  (System category, admin-only tile; hidden for a member session). One page,
  one Save: a three-button segment picks **Directory (LDAP / Active Directory)**,
  **Single sign-on (OpenID Connect)** or **Single sign-on (SAML)**, and the
  account list below is shared.
- **Backend:** `src/directory.rs` (model, validation, RFC 4515 escaping,
  group→role resolution, bounded cache, LDAP backend), `src/handlers/directory.rs`
  (HTTP surface + the login hook), `src/oidc.rs` (discovery, PKCE, token
  exchange, JWT/JWKS verification, settings persistence), `src/handlers/oidc.rs`
  (the seven OpenID Connect routes), `src/saml.rs` (XML tree, exclusive
  canonicalisation, signature and assertion verification, AuthnRequest,
  metadata), `src/handlers/saml.rs` (the seven SAML routes),
  `src/storage/directory_repo.rs` (the `remote_identities` rows and the mirrored
  `users` row), migration `072_directory.sql`.
- **Storage:** SQLite for the principals; `{DATA_DIRECTORY}/data/directory.json`,
  `{DATA_DIRECTORY}/data/oidc.json` and `{DATA_DIRECTORY}/data/saml.json` for the
  settings. The service-account password is sealed under the `directory-bind`
  scope and the OIDC client secret under `oidc-client-secret`, both in the same
  secret box as every other credential (`src/storage/secret_box.rs`). SAML has
  no secret to seal: the IdP certificate is public.
- **Dependencies:** `ldap3` with `tls-rustls-ring` for LDAP. OpenID Connect adds
  **no** new crate: `ring` signs and verifies, `base64` and `url` encode, and
  `reqwest` — already used by the rest of the appliance — fetches discovery,
  JWKS and the token endpoint. SAML adds **no** new crate either: `xml` (already
  in the tree through `xmltree`/`dav-server`) parses, `ring` verifies and
  digests, `x509-parser` reads the IdP certificate, `flate2` deflates the
  redirect binding.

## Endpoints

| Method | Path | Guard |
|--------|------|-------|
| `GET`    | `/api/directory` | `require_admin` — settings, health, principals, counts, break-glass |
| `PUT`    | `/api/directory/settings` | `require_admin` — save the whole document |
| `POST`   | `/api/directory/test` | `require_admin` — try the form (optionally a full sign-in) |
| `POST`   | `/api/directory/identities/{id}/disabled` | `require_admin` — `{"disabled": true\|false}` |
| `DELETE` | `/api/directory/identities/{id}` | `require_admin` — forget a principal, revoking its sessions |

OpenID Connect — the settings half is `require_admin`, the sign-in half is
public:

| Method | Path | Guard |
|--------|------|-------|
| `GET`    | `/api/oidc` | `require_admin` — settings, health, counts, break-glass |
| `PUT`    | `/api/oidc/settings` | `require_admin` — save the whole document |
| `POST`   | `/api/oidc/test` | `require_admin` — read the provider's discovery document, saving nothing |
| `GET`    | `/api/auth/methods` | **public** — `{"password": true, "oidc": {"enabled", "label"}, "saml": {"enabled", "label"}}` for the login screen |
| `POST`   | `/api/auth/oidc/start` | **public** — mints the state/nonce/PKCE flow and answers the provider URL |
| `GET`    | `/api/auth/oidc/callback` | **public** — the provider's redirect target; ends in a `303` |
| `POST`   | `/api/auth/oidc/complete` | **public** — trades the one-shot ticket for the session |

SAML 2.0 — same split:

| Method | Path | Guard |
|--------|------|-------|
| `GET`    | `/api/saml` | `require_admin` — settings, health, counts, break-glass |
| `PUT`    | `/api/saml/settings` | `require_admin` — save the whole document |
| `POST`   | `/api/saml/test` | `require_admin` — read the IdP's metadata document, saving nothing |
| `GET`    | `/api/saml/metadata` | `require_admin` — our service-provider metadata, for the admin to copy |
| `POST`   | `/api/auth/saml/start` | **public** — mints an AuthnRequest and answers the IdP URL |
| `POST`   | `/api/auth/saml/acs` | **public** — the IdP's POST target; ends in a `303` |
| `POST`   | `/api/auth/saml/complete` | **public** — trades the one-shot ticket for the session |
| `GET`    | `/api/auth/saml/metadata` | **public** — the same metadata, for an IdP that fetches it |

The `/api/auth/*` routes and `/api/auth/methods` are public because a signed-out
browser has to reach them; none of them accepts a credential that comes from the
request body alone (see *What happens on a single sign-on*). They are also the
only public routes here: the settings endpoints stay `require_admin`, and they
authenticate **before** parsing the body, so an anonymous visitor cannot use the
body parser as a format oracle.

Every mutation answers with the **full `GET` payload**, so the screen never has
to guess what a change did. The service-account password is the one field that
is write-only: the payload carries `has_bind_password`, never the password.

The save contract for that field is three-valued, which is what makes "leave it
alone" safe:

| `bind_password` in the body | Meaning |
|-----------------------------|---------|
| absent or `null`            | keep the sealed password |
| `""`                        | forget it |
| any other string            | seal and replace it |

`client_secret` follows the same three-valued rule on `/api/oidc/settings`.

## The three things that must agree

**1. The server.** `ldap://host:389` or `ldaps://host:636`. StartTLS is offered
for `ldap://` and refused for `ldaps://` (already encrypted). Certificate
verification is on by default; turning it off is allowed — a home NAS often
talks to a self-signed directory — and the screen says exactly what it costs
(anyone on the network could impersonate the server).

The **service account** is the read-only DN HomeVault searches with. An empty
bind DN means an anonymous search, which is allowed and documented; a bind DN
without a password is refused at save time rather than at 8 a.m. on Monday.

**2. The search.** A base DN plus two filters:

- `user_filter` **must** contain `{username}`; the login name is substituted
  through RFC 4515 escaping. A filter without the token is refused because every
  sign-in would then return the whole directory.
- `group_filter` is optional. Empty means "read the groups from the
  `memberOf` attribute"; a value must contain `{dn}` and is used as a second
  search with the user's DN substituted.

The attribute names (`uid`, `cn`, `mail`, `memberOf`) are editable, so FreeIPA,
OpenLDAP and AD all work. Switching the picker to **Active Directory** swaps the
*untouched* defaults for `sAMAccountName` / `displayName` / an AD user filter;
a value the admin typed is never overwritten.

**3. Roles from groups.** A table of `group → role`. The **highest** role wins,
so one admin group is enough; a principal in no mapped group gets
`default_role`. Matching is case-insensitive on the whole DN *and* on its
leftmost `cn`, because AD reports `memberOf` as full DNs while a hand-written
mapping often uses a bare group name — and silently failing to match one of the
two would be a security surprise in the shape of a demotion.

## What happens on a directory sign-in

1. `handle_login` reads `users.auth_source` for the submitted name. `local`
   means the directory is never consulted. Anything else means the directory
   owns the name. The decision comes from the database, **never** from the
   request body.
2. The integration must be on; off means a 401 that says "sign in with a local
   account" instead of "wrong password".
3. The per-IP login limiter still applies — an LDAP bind is a network round trip
   an attacker could otherwise hammer for free.
4. The service account binds, the user filter is rendered, and the search runs
   (or the cached search result is reused).
5. **The password is always verified with a fresh bind as the user's own DN**,
   cache hit or not. A cached entry can never become a credential.
6. The groups resolve to a role, the `remote_identities` row is upserted, and a
   mirrored `users` row is created or updated so sessions and foreign keys keep
   working.
7. A principal disabled on this appliance is refused **after** the password
   check, so disabling cannot be used to probe which names exist.
8. The session is created exactly like a local one (same token, CSRF, cookie,
   `remember_me`), and `must_change_password` is always false — a directory
   password is not HomeVault's to change.

A **local account is never sent to the directory**, and a directory account is
never checked against a local hash. The mirrored row carries the sentinel
`!directory-provisioned` in `users.password_hash`, which no password can match,
and its PAM fall-through is suppressed exactly like a local account's.

If a directory user tries to claim a name a local account already owns, the
sign-in is refused with a 403 — the directory cannot take over an existing local
identity.

## Single sign-on with OpenID Connect

The second provider. Same screen, same accounts list, same roles: only the way
the identity is proven changes.

### What the admin fills in

| Field | Meaning |
|-------|---------|
| Button text | What the login screen says — `Sign in with Authentik`, `Continue with Google`. Shown as typed, never assembled from a template |
| Issuer URL | Exactly what the provider reports as `issuer`, e.g. `https://auth.example.com/application/o/homevault` |
| Client ID | The application identifier registered at the provider |
| Client secret | Optional: a **public client** (PKCE only) leaves it empty |
| Callback URL | The exact address to register at the provider. Must end in `/api/auth/oidc/callback` |
| Claims / scopes | Behind *Advanced*: login name, display name, e-mail and groups claim names, plus the scopes (must contain `openid`) |

The callback URL is **explicitly configured**, never derived from the request's
`Host` header: a forged `Host` must not be able to move where a credential is
sent. It must be `https://`, and `http://` is accepted only for a loopback
address — which is what makes a first attempt on the NAS itself possible.

### What happens on a single sign-on

1. `POST /api/auth/oidc/start` mints a random `state`, a `nonce` and a PKCE
   verifier, stores them in a bounded, expiring pending store, sets the
   `hv_oidc_state` cookie (`HttpOnly`, `Path=/api/auth/oidc`, `SameSite=Lax`,
   600 s) and answers the provider's authorization URL. The client secret never
   reaches the browser.
2. The browser leaves for the provider. Anything the provider decides — consent,
   MFA, a password — happens there, not here.
3. `GET /api/auth/oidc/callback` requires the cookie to match the `state`, and
   the state to still be in the pending store. A callback without the cookie, a
   replayed state, or an error from the provider all land on
   `/login?sso_error=<code>` with a short code, never a token in a URL.
4. The code is exchanged at the token endpoint with `client_secret_basic` when a
   secret is stored, and with PKCE alone when it is not. The verifier sent is
   the one whose hash the browser was given.
5. The ID token is verified **before** anything is stored: signature against the
   provider's JWKS, `alg` ∈ {`RS256`, `ES256`, `EdDSA`} and matching the key type
   (`none` and HMAC are refused), `iss`, `aud`, `exp`/`iat`, and the `nonce` from
   step 1. The key set is refetched **once**, and only when the token names a
   `kid` the cached set does not contain.
6. The claims resolve to a login name (`sanitize_username`: an `@domain` suffix
   is dropped, anything outside `[A-Za-z0-9_]` becomes `_`, runs collapse,
   ≤64 chars, then the normal username rules — so reserved names like `admin`
   are refused *as a claim*, before the database is touched), a display name and
   an e-mail.
7. The groups resolve to a role with the **same** `resolve_groups_role` the LDAP
   path uses, the `remote_identities` row is upserted with `provider = 'oidc'`
   and the subject `issuer#sub`, and the mirrored `users` row is created or
   updated.
8. The session is created, then the callback answers `303` to
   `/login#sso=<one-shot-ticket>`. The ticket lives 120 s in a bounded store and
   is spent exactly once; `POST /api/auth/oidc/complete` trades it for the same
   `LoginResponse` a password login returns. **A token never appears in a URL**,
   and the fragment is wiped before the exchange starts.

### Refusals, and why each one is a refusal

| Situation | Result |
|-----------|--------|
| Callback without the `hv_oidc_state` cookie | `/login?sso_error=state` |
| Replayed or expired state | `/login?sso_error=expired` |
| Provider error (`access_denied`, …) | `/login?sso_error=denied` |
| Integration switched off mid-flow | `/login?sso_error=disabled` |
| Discovery/JWKS/token endpoint unreachable | `/login?sso_error=unavailable` |
| Bad signature, `alg: none`, wrong `iss`/`aud`/`nonce`, expired token, unpublished key | `/login?sso_error=refused` |
| Claim has no usable login name, or a reserved one | `/login?sso_error=claim` |
| Name already owned by a **local** account | `/login?sso_error=account` |
| Principal disabled on this appliance | `/login?sso_error=account_disabled` |
| Ticket missing, spent or too old | `401` from `/complete` |

Each code maps to one translated sentence on the login screen; the server never
sends free-form text through the URL.

### The mirrored account is not a local account

The provider stored on the account decides the login path, exactly as
`users.auth_source` does:

- An OIDC principal has `auth_source = 'directory'` and the sentinel
  `!directory-provisioned` hash, so the password form answers **403** — "This
  account signs in through single sign-on — use the single sign-on button" —
  instead of "wrong password".
- Conversely, the LDAP path refuses any principal whose stored provider is not
  `ldap`/`ad`. Two providers vouching for the same string are still two
  different principals, and a password typed for one must never open the other.
- `disabled` is deliberately **not** refreshed by a sign-in: an administrator
  switched that principal off, and a successful sign-in must not silently switch
  it back on.

## Single sign-on with SAML

The third tab speaks SAML 2.0 as a **service provider**: HomeVault publishes
metadata, the identity provider (Authentik, Keycloak, Entra ID, ADFS, Google
Workspace, …) posts a signed assertion back. It is the right choice when the
provider does not speak OpenID Connect, or when an existing IdP already holds
every group the household or the small office uses.

### What the admin fills in

- **Provider** — the IdP's entity ID and its single sign-on URL (where the
  AuthnRequest is sent), plus one or more **signing certificates**, one per line.
  The certificate is public: there is no client secret to seal for SAML.
- **Import** — pasting the IdP's **metadata address** and pressing *Read metadata*
  fills the three fields above; the result is shown but **nothing is saved**
  until the admin presses *Use these details*, and still nothing is written
  until *Save*. The address is fetched server-side over `https` only, with the
  same allow-list and body-size ceiling as every other outbound fetch.
- **Attributes** — which assertion attribute holds the display name, the email
  and the groups. Defaults are the usual `displayName`, `mail`, `memberOf`.
- **Group → role** — the same editor as the other two tabs; a matched group maps
  to Administrator, the highest match wins, and `default_role` applies when no
  group matches.
- **Our side** — the entity ID and the ACS URL are shown read-only, because the
  IdP has to be told exactly where to send people; the same values, and the
  signing certificate HomeVault expects, are in the copyable service-provider
  metadata (`GET /api/saml/metadata`).
- **On/off** — a switch. Off means the login screen does not offer the button and
  the ACS answers `disabled`, even for a valid assertion.

### What happens on a single sign-on

1. The login screen reads `/api/auth/methods`, sees `saml.enabled`, and renders a
   button labelled with the admin's own text (default "Sign in with single
   sign-on").
2. `POST /api/auth/saml/start` mints a request ID, remembers it with the
   `return_to` path and the browser's IP, sets the `hv_saml_state` cookie, and
   answers the IdP URL. Over `https` the cookie is `SameSite=None; Secure`
   (the ACS is a cross-site POST); over `http` it is `Lax` and the browser
   binding is simply not enforced — only a *wrong* cookie is refused.
3. The IdP authenticates the person and POSTs a `SAMLResponse` to
   `/api/auth/saml/acs`.
4. The ACS checks, in order: the integration is on; the request ID is one we
   issued (single use, bounded table, two-minute skew); the response is
   addressed to our entity ID and our ACS URL; the signature verifies against
   the configured certificate over the **canonicalised** XML; the assertion's
   `Conditions`/`NotOnOrAfter`/`SubjectConfirmationData` are inside the window
   and the audience is us; the login-name attribute is present and not reserved;
   the name is not owned by a local account; the principal is not disabled.
5. The person lands as a `remote_identities` row with `provider = 'saml'`, is
   mapped to a role by group, and the browser gets a session cookie plus a
   one-shot ticket that `/api/auth/saml/complete` trades for the SPA session —
   the same exchange the OpenID Connect flow uses, so nothing about the session
   layer changes.

### Refusals, and why each one is a refusal

| Situation | Result |
|-----------|--------|
| ACS without the `hv_saml_state` cookie (over `https`, and only when a cookie is present but wrong) | `/login?sso_error=state` |
| Replayed, unknown or expired request ID | `/login?sso_error=expired` |
| Integration switched off mid-flow | `/login?sso_error=disabled` |
| Bad signature, wrong issuer/audience/destination, expired or unsigned assertion, certificate not configured | `/login?sso_error=refused` |
| Login-name attribute missing, empty or reserved | `/login?sso_error=claim` |
| Name already owned by a **local** account | `/login?sso_error=account` |
| Principal disabled on this appliance | `/login?sso_error=account_disabled` |
| Ticket missing, spent or too old | `401` from `/complete` |

The signature is verified over **exclusive XML canonicalisation** of the signed
element, never over the raw bytes: an IdP is free to re-serialise, and a
byte-wise comparison would either refuse a valid assertion or — worse — accept
one whose signed bytes differ from the bytes the parser read. The parser refuses
a DTD outright, so entity expansion and external entities (XXE) cannot be used
to change what is verified.

## Break-glass: a local administrator always keeps their password

- The banner on the screen always shows how many **local administrators** can
  still sign in (`break_glass.local_admins`), and turns red at zero with the
  sentence that says so. That count exists so an admin sees the risk *before*
  saving, not during an outage.
- Turning either integration off, a wrong bind password, an expired certificate,
  an unreachable directory or an identity provider that is down **cannot** lock
  anyone out: local accounts never take a remote path, and the login screen
  always keeps its password form.
- The screen never enforces a minimum of local admins — it reports, it does not
  block — so an admin who deliberately runs a directory-only appliance is not
  fought by the UI.

## The directory cache

The cache holds **search results only** (DN, display name, e-mail, groups). It is
keyed by the configuration *and* the login name, so a changed server, base DN or
filter can never be answered by a result stored under the previous one.

- Entries: at most **512**; age: `cache_ttl_seconds`, **0–3600**, default 300.
- `0` disables the cache: every sign-in searches.
- A **wrong password drops the cached entry** — a stolen DN must not sit in
  memory after the directory entry is deleted.
- Saving settings, toggling a principal and running a test all invalidate it.
- The screen reports `hits` / `misses` / `entries` under *Advanced*.

## Directory validation and limits

Every bound is enforced on save **and** again on load and on each sign-in,
because `directory.json` is a file on disk and a hand-edited document must not
be able to widen anything.

| Field | Rule |
|-------|------|
| Provider | `ldap` or `ad` |
| Server URL | `ldap://` / `ldaps://`, a host, no query, no fragment, ≤512 chars |
| StartTLS | refused with `ldaps://` |
| Base DN | shape-checked DN (`type=value,…`), no control characters, ≤1024 |
| Bind DN | same shape; empty = anonymous search |
| Bind password | required when a bind DN is set; refused when a bind DN is empty |
| User filter | balanced, parenthesised, contains `{username}`, ≤512 |
| Group filter | empty, or balanced, parenthesised, contains `{dn}` |
| Attribute names | `[A-Za-z0-9-;.]`, ≤64 chars |
| Group mappings | ≤64, unique (case-insensitive), each needs a group, role ∈ {user, admin} |
| Cache TTL | 0–3600 s |

A **disabled** integration may be half-filled on purpose: an admin can prepare
the server, base DN and filters first and switch it on afterwards. Nothing is
contacted while it is off.

## Failure behaviour

| Situation | What the user sees |
|-----------|--------------------|
| Wrong password | `401 Invalid username or password`, and the failure counts toward the auto-block ladder |
| Directory unreachable / TLS error / bind failure | `503` with the reason, **no** ladder entry, **no** IP ban |
| Integration off, name only the directory knows | `401` "Directory sign-in is turned off — sign in with a local account" |
| Principal disabled here | `403` "This directory account is disabled on this appliance" |
| Principal claims a local name | `403` from the repository |
| Sealed password unreadable | `503`, naming the cause |
| Corrupt `directory.json` | falls back to "off": never a lockout |
| `POST /test` on a bad config | `400` with the exact sentence, nothing saved |
| Provider unreachable during an OIDC sign-in | `303` to `/login?sso_error=unavailable`; local accounts unaffected |
| Provider returns an error | `303` to `/login?sso_error=denied` |
| ID token refused | `303` to `/login?sso_error=refused`; no account, no session |
| One-shot ticket replayed | `401` from `/complete`; nothing is created |
| Corrupt `oidc.json` | falls back to "off": never a lockout |
| Bad or missing SAML assertion signature | `303` to `/login?sso_error=refused`; no account, no session |
| SAML assertion outside its window, replayed request ID, or signed for another destination | `303` to `/login?sso_error=expired` or `refused` |
| IdP metadata unreachable during *Read metadata* | `400` with the exact reason, nothing saved |
| SAML ticket replayed | `401` from `/complete`; nothing is created |
| Corrupt `saml.json` | falls back to "off": never a lockout |

An outage is not a wrong password: keeping it out of the lockout ladder is what
lets an admin retry as soon as the server is back, instead of waiting out a ban
they did not earn.

## Security notes

- **Least privilege:** every route is `require_admin`; the routes sit behind the
  same CSRF middleware as the rest of the admin API.
- **Secrets never leak:** the service-account password is sealed with
  AES-256-GCM under the `directory-bind` scope, never returned by any endpoint,
  never written to a log, and never echoed by the test panel. The panel sends
  the password only in the request body.
- **No filter injection:** every substituted value goes through
  `escape_filter_value` (RFC 4515 §3) — `*`, `(`, `)`, `\` and NUL are escaped,
  so a login name cannot widen the search.
- **Bounded work:** connect 5 s, every operation 8 s, cache capped in entries and
  age, group list capped at 256 before storage, ≤64 mappings.
- **Least data:** the group list stored is what the directory returned, no more;
  e-mail is stored because the users list shows it, not because the sign-in
  needs it.
- **Revocation is immediate:** disabling or forgetting a principal deletes its
  sessions and invalidates the cache; the next request from that account is
  refused.
- **SAML is parsed, never trusted:** a DTD is refused, so entity expansion and
  external entities cannot change the document; the signature is verified over
  exclusive canonicalisation of the signed element; the request ID is single use
  and expires; the audience, destination and recipient must all be us. The
  certificate list is public, capped, and an empty list refuses every assertion
  instead of accepting an unsigned one.
- **Authenticate before parsing:** the two mutating SAML settings routes check
  the admin session before they read the body, so an anonymous request gets
  `401`, never a body-format hint.

## Stages

ROADMAP-DSM P3.1 ships in three stages on this one screen:

- **Stage 1 — LDAP / Active Directory:** done (2026-11-01).
- **Stage 2 — OpenID Connect** (authorization code + PKCE, discovery,
  ID-token/JWKS validation): done (2026-11-01).
- **Stage 3 — SAML 2.0** (service-provider metadata, signed assertions): done
  (2026-11-02). No new crate: `xml` parses, `ring` verifies, `x509-parser` reads
  the IdP certificate, `flate2` inflates the redirect binding.

Each stage adds a provider to the same principal table (`provider` is a column)
and reuses the same `remote_identities` → role resolution, so a mixed appliance
(AD for staff, OIDC for a family, local for the admin) stays coherent.

## Related

- **Users** (`docs/guides/account.md`) owns local accounts, 2FA and the local
  password. A directory principal appears in the Users list as a mirror; its
  password is changed in the directory, and the change-password endpoint says so
  instead of "wrong current password".
- **Groups** (`docs/guides/groups.md`) owns sharing groups, which are unrelated
  to directory groups: those grant a **role**, not a share permission.
- **Security** (`docs/guides/security.md`) owns the login limiter and the
  auto-block ladder the directory path still respects.
- **Identity** (`docs/guides/identity.md`) owns passkeys and recovery for local
  accounts.
