# API Contract

All endpoints are mounted under `/api` by `src/web/mod.rs` (`create_app`). The
SPA's `ApiClient` (`frontend/src/lib/services/api.ts`) prefixes every call with
`/api`. Paths below are written **without** the `/api` prefix.

> This file is a curated map, not generated — `CLAUDE.md`'s engineering
> invariants are the current authority, with the per-feature detail in
> `guides/`. Reconciled with them **2026-09-08**.
>
> A machine-readable **OpenAPI 3** document is generated from the same route
> table (`GET /api/openapi.json`, administrator only). See
> [OpenAPI 3](#openapi-3--openapijson-admin) below; when this curated map and
> the generated document disagree about a *route*, the generated document wins
> because it is read from the router itself.

## Conventions

- **Request**: `POST`/`PUT` bodies are JSON with `Content-Type: application/json`.
  Uploads (`/explorer/upload`, avatar upload) use `multipart/form-data`.
- **Auth headers**: every authenticated request sends `X-Session-Token`. CSRF-
  protected mutations also send `X-CSRF-Token`.
- **Response envelope**: `ApiResponse<T>` = `{ success: bool, data?, error? }`
  (or `message` on some older handlers). On failure, `error`/`message` carries a
  human-readable string the client shows in a toast.
- **Auth levels** (see `src/handlers/admin_guard.rs`):
  - `auth` — any valid session (`require_auth`).
  - `admin` — admin session (`require_admin`).
  - `public` — no session required.
- **Auth status contract** (stated once, here and in
  `src/handlers/admin_guard.rs` — ROADMAP X.7 / X.8):

  | Situation | Status |
  |---|---|
  | `X-Session-Token` header absent or empty | `401` |
  | Token unknown or expired | `401` |
  | Valid session, account not admin (an `admin` route) | `403` |
  | Valid session, malformed/refused body | `400` |

  `400` is reserved for a request the server could not understand. It used to
  be the answer for a *missing* header on most of the API (each module rebuilt
  the check), so a client that branches on the status read "you are not signed
  in" as "your input is wrong"; and `shares`/`permissions`/`snmp`/`settings`
  answered `400` for a signed-in **non-admin** where every other module answered
  `403`. Every guarded endpoint now goes through `admin_guard`, which is the
  only place that turns the header into an identity decision.
- **Caching**: `ApiClient` caches successful `GET`s for a per-call TTL (default
  5 s) and invalidates cache keys on mutations.

#### ISO / IMG mounting — `explorer/iso` (phase B2)

Read-only loop mounts of a disc image, browsable as a folder.

| Method | Path | Auth |
|--------|------|------|
| GET | `/explorer/iso` | auth — `{ available, iso_root, mounts[] }`; `available: false` means `losetup`/`mount` are absent |
| POST | `/explorer/iso` | admin — `{ path }` → the mounted row `{ id, image_path, label, mount_point, loop_device, size_bytes }` |
| DELETE | `/explorer/iso/{id}` | admin |

Refusals are `400` when they are about the request (outside the caller's roots,
wrong extension, missing file, `..` in the path, unknown id) and `503` when the
host lacks `losetup`/`mount` — the capability contract below applies. The mount
point is **derived** (`<DATA_DIRECTORY>/iso/<slug>`), never taken from the
request, and the mount carries `ro,nosuid,nodev`. A failed mount detaches the
loop device it attached.

## Capability and unsupported-capability contract (P0.2)

HomeVault runs on hardware and against external tools it does not control:
`clamscan`, `docker`, `ufw`, `restic`, `targetcli`, `dnsmasq`, `upsd`, hwmon
PWM files, `zfs`/`btrfs`, `tc`, `miniupnpc`. A missing one is **not** a client
error: the request was well formed, the server simply cannot carry it out.
Reusing `400` for that made "fix your input" and "install a package"
indistinguishable to every caller.

**The rule: a missing capability is `503`; an unimplemented feature is `501`;
a malformed request stays `400`.**

| Situation | Status | `message` must say |
|---|---|---|
| A capability probe reports the tool/module/hardware absent | **`503`** | the **name of the missing piece**, so the screen can tell the user what to install |
| The route exists but this build cannot perform it at all | **`501`** | that it is not implemented, and what to do instead |
| Valid session, malformed or refused body | `400` | the field and the rule it broke |
| Missing/unknown session | `401` | see the auth contract above |
| Valid session, not an admin (an `admin` route) | `403` | `Admin privileges required for this operation` |

**Capability discovery is a `GET`, never a guess.** Every feature that depends
on something optional exposes what the host can do in its own read payload, and
the UI **hides the editor and names the missing piece** rather than letting
every save fail. The established field names:

| Field | Meaning |
|---|---|
| `available: bool` | the primary tool/daemon for this screen is present (`snmp`, `docker`, `ups-server`, `dhcp`, `syncthing`, `rclone`, `vm`) |
| `capabilities: { … }` | a per-feature object when several things can be missing at once — e.g. VPN's `wireguard`/`openvpn`, iSCSI's module + `targetcli` + configfs probes, traffic control's `tc_available`/`ifb_available` |
| `supports_<thing>: bool` | one narrow capability inside such an object (`supports_download_limit`, `supports_auto`) |
| `<thing>_available: bool` | one of several options is usable (traffic control's `ifb_available`) |

A `supports_*: false` is a statement about **what will work**, so a screen that
shows it must not offer the action either. The traffic-control card is the
reference: `supports_download_limit: false` because ingress shaping needs
`ifb`, and the screen says so instead of implying a download cap.

**The divergence is closed.** 38 call sites answered `400` for a missing tool
across `docker.rs` (26), `restic.rs` (3), `firewall.rs`, `antivirus.rs` and
`backup_target.rs`; all of them now go through
`ResponseHelper::json_unavailable` and answer **`503`**. A missing capability is
never `400` anywhere in the tree — `grep` for
`json_error("… is not available|not installed")` returns nothing.

`ResponseHelper::json_unavailable(missing)` and
`ResponseHelper::json_not_implemented(message)` are the two helpers a handler
uses; **new code must call one of them**, never `json_error`, when the reason is
"this host cannot" rather than "this request is wrong".

A tool name that lives inside an internal `Err(String)` (an archive extractor
naming the missing binary, for instance) still surfaces as the status of the
operation that failed — the rule applies to the *capability gate*, which is the
place a handler decides up front that it cannot proceed.

### Admin mutations, audit and `requires_restart`

- **Every mutation is `require_admin`; every read is `require_auth`.** A
  mutation that only *looks* harmless is still admin-only (e.g.
  `guided-redundancy/apply` is admin-only although it answers `501` today),
  because the privilege boundary must not have to be renegotiated when the
  implementation lands.
- **The guard runs before the body is parsed.** Handlers take a raw `Bytes`
  body and deserialize **after** `require_admin` succeeds, so an unauthenticated
  caller always gets `401` with the standard envelope and learns nothing about
  the payload shape. Extracting `Json<T>` first answers `422` and echoes the
  deserializer's message — that is a schema leak, and it was the Phase 4 defect.
- **A privileged action is journaled** through `crate::events::log_event`, and
  the line names the **account** and the **thing acted on**, never a secret:
  `"Admin 'x' deleted USB copy profile 'y'"`. Reads are not journaled.
- **A refused operation keeps the previous state.** Validation runs before
  persistence, so a save the host refuses never becomes the configuration a
  restart would re-apply. Where a write can still partially fail, the response
  names what did and did not happen (see `ups-server/shutdown`'s per-stage
  `skipped` flag).
- **`requires_restart: bool`** appears in the response of a mutation whose
  effect needs the process (or a listener) to rebind — today TLS. `true` means
  "saved, and it will not be live until the appliance restarts"; `false` means
  the change already took effect. A mutation that restarts the affected daemon
  itself reports `false` and says so in its own payload rather than implying a
  reboot. Absent **only** when the action can never need one.

### Secrets

- **A secret is never returned, in any response, in any form.** List and detail
  views answer a redacted projection: `redacted()` for DDNS, a per-tenant view
  that omits the token and the sealed password for backup targets, a boolean or
  a last-four hint elsewhere.
- **A secret at rest is sealed with the appliance key** and its scope as
  associated data (`directory-bind`, `oidc-client-secret`, `backup_target`'s
  repository password, iSCSI CHAP keyed on target IQN + initiator), never stored
  in clear and never logged.
- **A secret travels in a header, never a URL** — URLs land in logs, history and
  `Referer`. The configuration-backup passphrase is `x-backup-passphrase`.
- **"Shown once" is a real category**: an SSH key, a restic password or a
  rsync password is returned exactly once at creation and is unrecoverable
  afterwards; the screen says so before the user dismisses it.

## Auth — `auth/*`

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| POST | `/auth/login` | public | `handle_login` — 401 bad credentials, 429 per-IP throttle / account lockout; response includes `must_change_password` (admin-chosen password pending). A name whose `users.auth_source` is `directory` takes the directory path instead (see [Directory sign-in](./guides/directory-sso.md)): 503 when the directory is unreachable or the integration is off, with **no** lockout-ladder entry. A directory principal is never checked against a local hash, and a local account is never sent to the directory |
| POST | `/auth/logout` | auth | `handle_logout` |
| GET | `/auth/me` | auth | `handle_get_current_user` — includes `must_change_password` |
| POST | `/auth/2fa/verify` | public | `handle_2fa_verify` |
| POST | `/auth/2fa/setup` | auth | `handle_2fa_setup` |
| POST | `/auth/recovery-code` | auth | `handle_generate_recovery_code` (400 if one exists unless `?regenerate=true`) |
| POST | `/auth/verify-recovery-code` | public | `handle_verify_recovery_code` — `{ username, code }` |
| POST | `/auth/reset-password-recovery` | public | `handle_reset_password_recovery` — code is not consumed |
| POST | `/auth/recover` | public | `identity::handle_recover_request` — `{ username }` → `{ sent: true }` with the same answer whether or not the account exists, has a rescue address, or was throttled (one mail per account per minute) |
| POST | `/auth/recover/verify` | public | `identity::handle_recover_verify` — `{ username, code }` → `{ username, recovery_code }`; the six-digit code is single use and destroyed after five wrong attempts |
| GET | `/auth/setup-status` | public | `handle_setup_status` — `is_first_run` (DB-derived), `is_setup_valid`, `hardware_changed`, `needs_recovery`, `server_machine_id`, `sudo_available`, `system_language` (host POSIX locale → `en`/`fr`/`de`/`es`) |
| GET | `/auth/check-username` | public (local network only, first-run only) | `handle_check_username` — 403 once an admin exists (post-setup it would be a free username oracle) |
| POST | `/auth/setup` | public (local network only) | `handle_setup` — accepts optional `totp_secret` + `totp_code` (enrolled at the wizard's password step); response may include `totp_enabled` + `backup_codes`. Refused once an admin exists |
| POST | `/auth/setup/verify-totp` | public (local network only, first-run only) | `handle_setup_verify_totp` — stateless pre-creation check of a client-generated TOTP secret: `{ secret, code }` → `{ valid }` |
| POST | `/auth/setup/rebaseline` | admin | `handle_setup_rebaseline` — re-baseline the hardware fingerprint after a confirmed hardware change |

The three unauthenticated bootstrap routes (`/auth/setup`,
`/auth/setup/verify-totp`, `/auth/check-username`) accept only loopback /
private / link-local peers (`middleware::setup_guard`), **only same-origin
browser contexts** (a foreign `Origin` or `Sec-Fetch-Site: cross-site` → `403`),
and are **rate-limited to 20 requests/min per peer** (`429`).
`HOMEVAULT_SETUP_ALLOW_REMOTE=1` lifts only the network check.
All three are also **first-run-only at the handler level**: once an admin
exists `/auth/setup` refuses ("Setup already complete"), and
`/auth/setup/verify-totp` + `/auth/check-username` answer `403`.
`/auth/setup/progress` sits behind the same network + same-origin guard but is
**exempt from the 20/min budget** — it is the wizard's own step bookkeeping
(one PUT per navigation), and metering it made the wizard exhaust its budget
and starve its own `check-username` calls.
`/auth/setup-status` is exempt from all of it (public by design, read by the
login screen).

Login returns a session token (stored as `session-token` in localStorage) and
sets an `hv_session` cookie read only by the app-proxy auth entry point:
`GET /app-auth?app=<store>:<app>` (browser navigation, unauthenticated by
design — it is the redirect target of a gated app's proxy). It validates the
`hv_session` cookie, mints an app-proxy-scoped `hv_app_session` and redirects
to the app's proxy port; under an HTTPS deployment the gate's redirect targets
the TLS entry, because the `Secure` session cookie would never travel over
plain HTTP (see [Apps Center](./guides/apps-center.md)).
`POST /auth/setup` returns the same session shape plus `totp_enabled` and
`backup_codes` when TOTP was enrolled at the wizard's password step.
2FA-enabled accounts: `/auth/login` returns `requires_2fa`; the TOTP code is sent
to `/auth/2fa/verify`. The old email `/auth/forgot-password` + `/auth/reset-password`
flow was **removed** 2026-09-03 — see [Authentication](./guides/auth.md).

## Directory and single sign-on — `directory/*`, `oidc/*`, `saml/*`, `auth/oidc/*`, `auth/saml/*`

Three providers, one principal model. The settings halves are `admin`; the
`/auth/*` halves **are** the sign-in and are `public` by design. Full behaviour,
refusal table and failure modes: [Directory and single sign-on](./guides/directory-sso.md).

| Method | Path | Auth | Handler / notes |
|--------|------|------|-----------------|
| GET | `/directory` | admin | `handle_directory_get` — settings (password write-only, `has_bind_password`), live `status` (`enabled`, `usable`, `problem`, cache hits/misses/entries), the mirrored identities, `counts`, and `break_glass.local_admins` |
| PUT | `/directory/settings` | admin | `handle_directory_settings_put` — the whole document; `bind_password` is three-valued (`null`/absent keeps the sealed one, `""` clears it, any other string seals and replaces it); answers the full `GET` payload |
| POST | `/directory/test` | admin | `handle_directory_test` — reads the form as typed, saves nothing; with a `username`/`password` it also reports the role that sign-in would grant |
| POST | `/directory/identities/{id}/disabled` | admin | `handle_identity_disabled` — `{ disabled }`; disabling also ends that principal's sessions, the directory is untouched |
| DELETE | `/directory/identities/{id}` | admin | `handle_identity_delete` — forgets the mirrored principal and its shadow `users` row |
| GET | `/oidc` | admin | `handle_oidc_get` — same shape as `/directory` for the OpenID Connect half (`has_client_secret`, no secret ever returned) |
| PUT | `/oidc/settings` | admin | `handle_oidc_settings_put` — `enabled`, `label`, `issuer`, `client_id`, `client_secret` (three-valued like `bind_password`), `redirect_url`, `scopes`, the four claim names, `default_role`, `group_roles` |
| POST | `/oidc/test` | admin | `handle_oidc_test` — fetches the provider's discovery document with the form as typed, saves nothing |
| GET | `/auth/methods` | public | `handle_auth_methods` — `{ password: true, oidc: { enabled, label }, saml: { enabled, label } }`, what the login screen reads to decide which buttons to show |
| POST | `/auth/oidc/start` | public | `handle_oidc_start` — mints `state`, `nonce` and a PKCE verifier, sets the `hv_oidc_state` cookie (`HttpOnly`, `Path=/api/auth/oidc`, `SameSite=Lax`, 600 s) and answers `{ authorize_url, state }` |
| GET | `/auth/oidc/callback` | public | `handle_oidc_callback` — the provider's redirect target; verifies the cookie/state, exchanges the code, verifies the ID token, upserts the principal, creates the session and answers **`303`** to `/login#sso=<one-shot-ticket>`. Every failure is a `303` to `/login?sso_error=<code>` |
| POST | `/auth/oidc/complete` | public | `handle_oidc_complete` — `{ code }` → the same `LoginResponse` a password login returns. The ticket lives 120 s in a bounded store and is spent exactly once; a missing/spent/expired one is `401` |
| GET | `/saml` | admin | `handle_saml_get` — same shape as `/directory` for the SAML half: `settings`, `status` (`enabled`, `usable`, `problem`, `pending_requests`), `counts`, `break_glass.local_admins` |
| PUT | `/saml/settings` | admin | `handle_saml_settings_put` — the whole document: `enabled`, `label`, `idp_metadata_url`, `idp_entity_id`, `idp_sso_url`, `idp_certificates`, `sp_entity_id`, `acs_url`, the three attribute names, `default_role`, `group_roles`. Authenticates **before** parsing the body |
| POST | `/saml/test` | admin | `handle_saml_test` — fetches and parses the IdP's metadata document with the form as typed, saves nothing. Authenticates before parsing |
| GET | `/saml/metadata` | admin | `handle_saml_metadata` — our service-provider metadata (`EntityDescriptor`, `SPSSODescriptor`, signing certificate), for the admin to copy into the IdP |
| POST | `/auth/saml/start` | public | `handle_saml_start` — mints a request ID, sets the `hv_saml_state` cookie and answers `{ authorize_url, request_id }` |
| POST | `/auth/saml/acs` | public | `handle_saml_acs` — the IdP's POST target; verifies the signed assertion and answers **`303`** to `/login#sso=<one-shot-ticket>` or `/login?sso_error=<code>` |
| POST | `/auth/saml/complete` | public | `handle_saml_complete` — `{ code }` → the same `LoginResponse`; one-shot, 120 s, `401` when missing/spent/expired |
| GET | `/auth/saml/metadata` | public | the same service-provider metadata, for an IdP that fetches it itself |

Both settings documents are persisted to `{DATA_DIRECTORY}/data/directory.json`
and `…/oidc.json`; the service-account password is sealed under the
`directory-bind` scope and the client secret under `oidc-client-secret`
(`src/storage/secret_box.rs`), and neither is ever returned. SAML settings live
in `…/saml.json` and hold no secret — the IdP certificate is public. Every
mutation answers with the full `GET` payload so the screen never guesses what a
change did.

**Who may sign in** is decided server-side and never by the request body:
`users.auth_source = 'directory'` (and the provider stored on the principal)
selects the remote path, so a local account is never sent to a provider and a
remote account is never checked against a local hash. `POST /auth/login` on an
OIDC principal answers `403` ("This account signs in through single sign-on —
use the single sign-on button"), and the LDAP path refuses any principal whose
provider is not `ldap`/`ad`. A provider outage is a `503` (or a `303` to
`sso_error=unavailable`) with **no** lockout-ladder entry and no IP ban, and
local accounts are never affected — the break-glass rule.

The `redirect_url` is configured explicitly, never derived from the request's
`Host` header; it must be `https://` (`http://` only for a loopback address) and
must end in `/api/auth/oidc/callback`. ID tokens are accepted only with `RS256`,
`ES256` or `EdDSA` and a matching key type (`none` and HMAC are refused) after
`iss`, `aud`, `exp`/`iat` and `nonce` checks; the JWKS are refetched once, and
only for a `kid` the cached set does not contain.



## Settings — `settings/*`

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/settings` | auth | aggregate of all categories |
| GET | `/settings/public` | public | safe subset (server name, language, version, `terminal_enabled` — no appearance) |
| GET/PUT | `/settings/general` | auth / admin | server name, timezone, `mdns_enabled`, `terminal_enabled` — the timezone is validated (IANA) and applied to the host with `timedatectl` (a failed apply returns `timezone_notice`); `mdns_enabled` drives the Avahi service file and the `host-name` override (a refused host change returns `mdns_notice`). The `GET` reports the **live** `mdns_enabled`; `terminal_enabled` (default true) refuses new terminal tickets when false |
| GET | `/settings/timezones` | auth | every zone name the scheduler can evaluate (`chrono-tz`'s IANA database, region-prefixed names + `UTC`), sorted — what the Settings picker renders |
| GET/POST | `/settings/language` | auth | set UI language — persisted as `general.language` and applied at boot before the first page renders; unsupported tags answer `400` |
| GET | `/translations` | auth | returns flattened `locale → key → string` map (`handle_get_translations`, `src/handlers/settings.rs`; mounted at `/api/translations`) |
| GET/PUT | `/settings/email` | auth / admin | SMTP relay; validated before it is stored (From: address, port) and used to generate the `msmtp` configuration |
| POST | `/settings/email/test` | admin | send test email |
| GET/PUT | `/settings/notifications` | auth / admin | alert e-mail, threshold, `webhook_url`/`webhook_enabled`/`webhook_format`; the URL must be `http(s)` with a host and no embedded credentials (`400` otherwise) and is validated again at dispatch (`validate_webhook_url`) |
| GET | `/settings/appearance` | any session | the appliance look: `{ wallpaper, accent_color }` only — read by every account's boot hydration (ROADMAP 26.1) |
| PUT | `/settings/appearance` | admin | `wallpaper` + `accent_color`, validated before anything is stored |

**`PUT /settings/appearance` validation (both fields optional, both checked
first — a refusal writes nothing):**

- `wallpaper` — `""`, `/wallpapers/hv-NN.webp`, `hv-NN` (1–44), a bare JSON
  string, or the `WallpaperConfig` JSON `{"type":"image","value":…}` (legacy
  `{"type":"default"}` → `hv-01`). Always stored as the bare
  `/wallpapers/hv-NN.webp` URL.
- `accent_color` — `#rgb`, `#rrggbb` or `#rrggbbaa` (the legacy `#4a90d9` is
  rewritten to `#1E62AE`).

A refusal is `400` with `{"success": false, "message": "wallpaper: '…' is not
one of the 44 bundled wallpapers (…)"}` — the message starts with the field
name. `theme` and `density` no longer exist on this resource (ROADMAP 26.2);
an old client that still sends them is accepted and they are ignored. |

| GET/PUT | `/settings/update` | auth / admin | update-check frequency |
| GET | `/settings/version` | auth | current version + last cached check + `last_update` (`status`, `version`, `previous_version`, `message`, `finished_at`; `null` until the installer's signed/atomic engine ran) — never forces an outbound call |
| POST | `/settings/version/check` | admin | "Check now" — the only path that forces the outbound update check |
| GET | `/settings/export` | admin | JSON dump, stamped with `schema_version` (SMTP password redacted) |
| POST | `/settings/import` | admin | JSON import — refused when `schema_version` is newer than this build or any field is invalid, naming the field |
| POST | `/settings/reset` | admin | reset to defaults, including antivirus / anti-ransomware / disk spindown / DB-backup settings and `ups.conf`; a domain that failed is listed in `warnings` |
| GET/PUT | `/settings/tls` | admin | built-in HTTPS config + cert info; PUT returns `requires_restart`. `mode` is `self-signed` \| `manual` \| `acme`; `acme` is refused until a certificate was actually issued |
| POST | `/settings/tls/regenerate` | admin | force self-signed cert regeneration |
| GET | `/settings/config-backup` | admin | full configuration backup state: `app_version`, `format_version`, `settings_schema`, `max_archive_bytes`, the config allow-list, the archives on disk, `pending_restore` |
| POST | `/settings/config-backup/export` | admin | `{ include_secrets, passphrase? }` → `{ id, bytes, path, created_at, encrypted, skipped }`; `include_secrets` requires a passphrase of 12+ characters (`400` otherwise, nothing written) |
| GET | `/settings/config-backup/archives/{id}` | admin | the archive (`application/octet-stream`, attachment, `no-store`); the id must match `[A-Za-z0-9_-]{1,64}` **and** resolve inside the archive directory |
| POST | `/settings/config-backup/inspect` | admin | dry run: archive as the raw body, passphrase in `x-backup-passphrase` → the manifest, or the refusal. Nothing staged |
| POST | `/settings/config-backup/restore` | admin | validates, stages into `data/restore/`, answers `{ staged: true, requires_restart: true, staged_entries, needs_root, manifest }` — applied at the next start, with the old database kept as `homevault.db.pre-restore` |
| GET/PUT | `/settings/acme` | admin | certificate automation: `enabled`, `domain`, `contact_email`, `directory_url`, `tos_agreed`, `challenge` (`http-01`/`dns-01`), `provider` (`manual`/`cloudflare`), `dns_api_token` (write-only: `"set"` keeps, `""` clears), `dns_propagation_seconds`; response adds the derived `status` (`disabled`/`idle`/`pending`/`valid`/`expiring`/`error`), `last_error`, `last_attempt`, `last_success`, `pending_dns_record`, `cert_info`. A change to domain / challenge / provider / directory URL resets `consecutive_failures` and `last_error` |
| POST | `/settings/acme/issue` | admin | start an issuance in the background → `{started:true}`; `400` when disabled, invalid, a DNS record already waits, or a run is already going (never queued) |
| POST | `/settings/acme/confirm` | admin | resume the pending manual DNS-01 run → `{started:true}`; `400` when nothing waits |
| POST | `/settings/acme/test-challenge` | admin | publish a throwaway token and fetch it over plain HTTP at the domain → `{reachable, url, status?, error?}`; the port-80 check that happens before an authority fails an order |
| GET | `/settings/ddns` | admin | dynamic DNS: `enabled`, `provider` (`duckdns`/`dynv6`/`cloudflare`/`ovh`/`namecheap`/`desec`/`custom`), `hostname`, `token` (write-only — `"set"` or `""`), `username` (`ovh` DynHost login), `domain` (`namecheap` registered domain), `custom_update_url`, `status`, `last_ip`, `last_update`, `last_error`, plus the read-only `current_ip` and `providers[]` |
| PUT | `/settings/ddns` | admin | save; validated only when `enabled` (a disabled form may be half-filled), token kept on `"set"`, recorded result cleared when the name, provider, OVH username or Namecheap domain changes |
| POST | `/settings/ddns/update` | admin | send an update now regardless of the change check; `400` when disabled, with the provider's own reason on failure |
| GET | `/settings/proxy` | admin | outbound HTTP(S) proxy: `enabled`, `http_proxy`, `https_proxy`, `no_proxy`, plus the read-only `effective_no_proxy` (the saved list **plus** loopback, link-local, RFC 1918, IPv6 ULA/link-local and `.local`), `supported_schemes` (`http`/`https`) and `requires_restart` (`false`) |
| PUT | `/settings/proxy` | admin | save; `http://`/`https://` only, credentials in the URL refused, at least one URL required while enabled, a disabled entry may stay empty; applied to new outbound calls without a restart |
| GET | `/settings/syslog` | admin | remote syslog forwarding: `enabled`, `protocol` (`udp`/`tcp`/`tls`), `host`, `port`, `format` (`rfc5424`/`rfc3164`), `facility` (0-23), `ca_pem` + `ca_set` (TLS only, pinned), `include_app_events`, the derived `status` (`disabled`/`idle`/`forwarding`/`degraded`), `last_sent_at`, `last_error`, and the live `queue` (`pending`, `enqueued`, `dropped`, `sent`, `failed`) |
| PUT | `/settings/syslog` | admin | save; validation only while `enabled` (a disabled entry may stay half-filled). TLS requires a PEM CA. `status`/`last_error`/`last_sent_at` are server-owned and **ignored** on write; changing the destination clears the recorded outcome |
| POST | `/settings/syslog/test` | admin | sends one real message now: `200` on success ("sent, check the collector" — UDP is fire-and-forget), `400` with the collector's own reason otherwise, and `400` while forwarding is disabled |
| GET | `/settings/reachability` | admin | read-only ordered report `{checks:[{id,state,detail,fix}], state, counts:{ok,warn,fail}}`; `id` is stable API (`tls.enabled`, `tls.trusted_certificate`, `acme.automation`, `ddns.enabled`, `names.agree`, `ports.https`, `ports.http01`, `tls.mode`) |

**Outside `/api`:** `GET /.well-known/acme-challenge/{token}` serves the HTTP-01
answer from a process-wide registry (no session, no CSRF, no contract layer — an
authority fetches it anonymously). A miss is a bodyless `404`; an answer carries
`Cache-Control: no-store` and a token with anything but `[A-Za-z0-9_-]` is
treated as a miss, never as a path.
| GET | `/settings/backup/db` | admin | snapshots of HomeVault's own database — settings, files on disk, total size and `last_error` (the reason the last attempt failed, cleared by the next success). The DB is WAL, so the `.db` alone is not restorable |
| PUT | `/settings/backup/db` | admin | update `interval_hours` / `retain`; a lower retention prunes immediately. `retain` bounds regular snapshots only — a `.pre-migration` restore point is never pruned by it and is bounded separately (3) |
| POST | `/settings/backup/db/now` | admin | take a `VACUUM INTO` snapshot now → `{ backup, status }`. Serialised process-wide: it waits for a snapshot already running instead of starting a second `VACUUM INTO` |
| GET | `/settings/backup/restore-drill` | admin | last non-destructive restore drill (`report` or `null`). The report carries `ready` plus the `database`, `config` and `data_backup` checks (**P2-9**) |
| POST | `/settings/backup/restore-drill` | admin | run the drill now: copy the newest DB snapshot to a temp dir, `PRAGMA integrity_check` read-only, decode the newest configuration archive (optional `x-backup-passphrase` header), report the last restic job. Never writes to the data directory |
| GET/PUT | `/settings/autostart` | admin | start-on-boot (systemd) for `homevault.service`. Both calls are bounded (`run_timed`) and run off the async runtime. GET answers `200 {enabled}` or `500 {success:false, message:<host reason>}` when the host cannot answer (container / no systemd / unit missing) — it never reports a false `enabled:false`. PUT answers `200 {enabled, message}` or `500` with the host's own stderr (`polkit refused: …`, `sudo: a password is required`, `Failed to get unit file state…`); a refusal changes nothing |

## Passkeys — `passkeys/*` (identity)

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/passkeys` | auth | the caller's passkeys (`credential_id`, `label`, `algorithm`, `attestation_fmt`, `user_verified`, `created_at`, `last_used_at`), plus `supported` and `relying_party {id, origin}`. **No key material** is ever in the payload |
| POST | `/passkeys/register/begin` | auth | WebAuthn creation options: `challenge` (single use, 5 min), `rp`, `user` (a base64url user handle), `pub_key_cred_params` (`-7` ES256 then `-8` EdDSA), `attestation: "none"`, `authenticator_selection`, `exclude_credentials`. Refused with a named reason when the account already holds 20 passkeys |
| POST | `/passkeys/register/finish` | auth | `{client_data_json, attestation_object, credential_id, label?}` (base64url). Verifies the ceremony and stores the credential. `400` naming the rule on any failed check; the challenge is consumed whatever the outcome |
| POST | `/passkeys/login/begin` | public | `{username?}` → `{challenge, rp_id, timeout, allow_credentials[], user_verification}`. An unknown username gets the same shape (a real challenge, an empty list) — no enumeration oracle. Applies the login auto-block |
| POST | `/passkeys/login/finish` | public | `{credential_id, client_data_json, authenticator_data, signature, username?, remember_me?}` → the same `LoginResponse` as `POST /auth/login` on success. `401` with one sentence ("Passkey sign-in failed") for an unknown credential, a bad signature or a counter regression — never a hint about whether the account exists |
| PATCH | `/passkeys/{id}` | auth | `{label}` — rename one the caller owns; `404` for anyone else's |
| DELETE | `/passkeys/{id}` | auth | revoke one the caller owns; `404` for anyone else's |
| POST | `/passkeys/sweep` | admin | drop the expired challenges now → `{removed}` |

The relying party (`rp_id` / origin) is derived from `HOMEVAULT_HOSTNAME`, then the ACME domain, then the machine's host name — **never** from the request's `Host`/`Origin` header. The origin carries the port when it is not the scheme default (HomeVault's HTTPS listener is 8443).

## Explorer tags — `explorer/tags` (session)

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/explorer/tags` | session | own tags; optional `?path=` is canonicalized and checked against the session's Explorer roots |
| POST | `/explorer/tags` | session | `{ path, tag }`; canonical accessible existing path, 1–32 visible tag characters, up to 20 tags per path; duplicate tag is idempotent |
| DELETE | `/explorer/tags` | session | `{ path, tag }`; removes only the caller's tag at the accessible path |

Tags are private metadata, not grants. Deleting a user purges their rows. The global search's optional `tag` parameter filters already-confined hits against the caller's own tags.

## Global search — `search/*` (session)

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/search/files` | session | `handle_search_files` — cross-root **file name** search for the ⌘K overlay (QW-12). Query: `q` (required, 2–128 characters, trimmed and lowercased, no `/`, `\` or NUL), `limit` (clamped to `1..=100`, default 30), `files_only` (drops directories), optional `tag` (matches the caller's own tags). Answers `{ query, hits[], roots[], timed_out[], unreadable[], truncated, elapsed_ms }` (and echoes `tag` only when supplied); each hit is a `FileInfo` plus `root` and `root_kind`. Fanned out over the same roots the caller's Explorer sidebar lists (`explorer_roots_for_session`), so an unauthorised folder can never produce a hit; **admins are bounded to those roots too**. Tag filtering is applied only after those confined hits are found. Bounded end to end: 16 roots max, 1.5 s per root, 4 s total — a root that does not answer is named in `timed_out` rather than dropped. A refused query is a `400` naming the rule, never a `200` with an empty list. File **names** only; content matches come from `/search/content` (phase D1, opt-in) |
| GET | `/search/content?q=&limit=` | session | `handle_content_search` — FTS5 full-text matches when the administrator enabled the content index. Answers `{ available, query, hits[] ({ path, name, size_bytes, modified, snippet, root, root_kind, root_name }), truncated, elapsed_ms }`. Filtered by the caller's `explorer_roots_for_session` **before** any snippet is built; `available: false` while the index is off. Same query rules as `/search/files`, plus at most eight quoted tokens (FTS5 operators are treated as text) |
| GET | `/search/index` | admin | `handle_content_index_get` — `{ config, candidates[], state {running,last_run,last_error,truncated}, docs, bytes, limits }`; `candidates` are the appliance's canonical Explorer roots |
| PUT | `/search/index` | admin | `handle_content_index_update` — `{ config }` with `{ enabled, roots[], exclude_names[], exclude_extensions[], retention_days, confirmed }`; enabling requires `confirmed: true`, each root must be a candidate or below one, 16 roots max, retention 1–3650 days |
| POST | `/search/index/run` | admin | `handle_content_index_run` — starts a rebuild, `202` with `{ started }`; `400` while disabled |
| DELETE | `/search/index?confirm=true` | admin | `handle_content_index_delete` — deletes every document and disables the feature; `400` without `confirm=true` |

## Security — `security/*` (auth)

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/security/advisor` | admin | the read-only hardening audit (QW-10): `score` (0–100, weighted and floored), `counts` (`pass`/`info`/`warn`/`fail`/`total`), `findings[]` (worst first: `id`, `severity` `pass`/`info`/`warn`/`fail`, `title`, `evidence`, `fix`, `link` `{app, category}`), `generated_at`, `app_version`. **There is no write route** — the advisor never changes a setting, every fix is a deep link. Never cached: a fixed finding is gone on the next call. A signal that cannot be read is an `info` finding, never a pass |
| GET | `/security/sessions` | auth | `handle_get_my_sessions` |
| POST | `/security/sessions/revoke` | auth | `handle_revoke_sessions` |
| POST | `/security/sessions/revoke-all` | auth | `handle_revoke_all_sessions` |
| GET | `/security/connected-clients` | admin | who is connected over the file protocols: `clients[]` (`id`, `protocol` `smb`/`nfs`/`sftp`/`ftp`, `username`, `client_ip`, `client_host`, `target` (share or export), `opened_files` **count** — never a path — `kickable`, `detail`), `sources[]` (`source`, `available`, `error`, `count`), `kickable`, `checked_at`. **Always `200`**: an unavailable collector is reported, not turned into an error |
| POST | `/security/connected-clients/kick` | admin | `{ id }` — disconnect one client. The body names a **client id, never a pid**: the server re-collects and takes the pid from the fresh observation. SMB uses `smbcontrol <pid> close-share <share>`, SFTP/FTP a `TERM` to that connection's process; an NFS export cannot be dropped (`400` with the reason) |
| GET | `/security/auto-block` | admin | login auto-block: `settings` (`enabled`, `threshold`, `window_seconds`, `ban_seconds`, `apply_firewall`, `dos_enabled`, `dos_requests_per_minute`), `bans[]` (active first; `ip`, `reason`, `failures`, `expires_at`, `firewall_rule`, `active`, `remaining_minutes`), `active`, `firewall_rules`, `firewall_available` (whether `ufw` exists), `limits` |
| PUT | `/security/auto-block` | admin | save the policy (a flat optional subset); validated only while `enabled` (a zero threshold/window/ban or a zero DoS cap is a `400` naming the field), then clamped into range |
| POST | `/security/auto-block/pardon` | admin | `{ ip }` — lifts the row **and** the `ufw` rule it installed; a block that is not there is a `400`; if the row was removed but its firewall rule could not be, that is reported instead of claimed as success |
| POST | `/security/auto-block/clear` | admin | lifts every block → `{ lifted, state }` |
| POST | `/security/auto-block/sweep` | admin | expires the due bans now → `{ removed, state }` (the same sweep runs on every login check) |
| GET | `/security/login-history` | admin | `handle_get_login_history` — truthful rows: each recorded attempt carries the real `success` flag and the HTTP `User-Agent`; rate-limited failures are kept (they are security-relevant) but the per-user counters only count `success = 0` attempts, and `clear_attempts` only purges failures — successful logins stay in history |
| GET | `/security/login-history/{username}` | admin | `handle_get_user_login_history` — same truthful success/user-agent per row |

## Identity — `identity/*` (auth)

Every route acts on the caller's **own** account: the username comes from the
session, never from the body, so there is no id to tamper with. A stray device
cookie (or a JWT for another account) is a miss, not a cross-account action.
See [Identity](./guides/identity.md).

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/identity/devices` | auth | `{ devices: [{id, label, created_at, last_seen_at, expires_at}], total, lifetime_days }` — never the token or its hash |
| POST | `/identity/devices/revoke` | auth | `{ id }`; revoking another account's device is a `400` miss ("no such trusted device") |
| POST | `/identity/devices/revoke-all` | auth | `{ revoked: n }` |
| GET | `/identity/rescue-email` | auth | `{ configured, email, code_ttl_minutes }` — `configured` only once a code came back |
| POST | `/identity/rescue-email` | auth | `{ email }`; validated, a code is mailed, and **nothing is stored** yet. `400` when the mail could not be sent |
| POST | `/identity/rescue-email/verify` | auth | `{ code }` → `{ email }`; the address is stored and the code consumed (single use, 5 attempts, 15 min) |
| DELETE | `/identity/rescue-email` | auth | remove the address and any outstanding code |
| POST | `/auth/login` | public | an account with 2FA skips the second factor when the request carries a valid `hv_trusted_device` cookie for **that** account (`requires_2fa` otherwise) |
| POST | `/auth/2fa/verify` | public | accepts `remember_device`; `true` adds `Set-Cookie: hv_trusted_device=…` (HttpOnly, SameSite=Strict, 30 days, `Secure` only over TLS) |
| POST | `/auth/2fa/setup` (`disable`) | auth | also revokes every trusted device of the account |

## Dashboard & widgets

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/dashboard/data` | auth | CPU/RAM/disk/network summary; `widgets` is **the caller's own grid** |
| GET | `/dashboard/widget/{id}` | auth | live data for one tile type (`cpu`, `memory`, `uptime`); **`404`** for any other id |
| POST | `/dashboard/widget` | auth | add a tile to the caller's grid — `{id, wtype, title, size, row, col, row_span?, col_span?}`, strictly validated (`400` on a bad slug/title/`size`/position, `400` "already exists" for an id the caller already owns, `400` past 64 tiles) |
| DELETE | `/dashboard/widget/{id}` | auth | remove one of the caller's tiles; **`404`** when the id is not theirs (another account's tile is indistinguishable from a missing one) |
| POST | `/dashboard/widgets/reorder` | auth | `{widgets: [id…]}` → index → 3-column row/col; ids the caller does not own are ignored, a list matching none of their tiles is **`404`** |
| GET | `/dashboard/widgets` | auth | the caller's tiles in display order |
| GET | `/dashboard/network-traffic` | auth | |
| GET | `/dashboard/cpu-temperature` | auth | |
| GET | `/dashboard/storage-volumes` | auth | |
| GET/POST | `/dashboard/widget-preferences` | auth | floating-panel position/visibility, keyed by the resolved username; `400` on a bad `widget_key` or an off-screen position, `401` for an invalid **or expired** session (it used to answer `400` and accept an expired token) |
| GET | `/dashboard/disk-io` | auth | |
| GET | `/dashboard/storage` | auth | |
| GET | `/dashboard/health` | auth | |
| GET | `/dashboard/server-details` | auth | motherboard/BIOS model |
| GET | `/health` | auth | overall health + CPU/mem from `/proc` |
| GET | `/health/live` | public | liveness probe — `200 {"status":"ok"}` whenever the HTTP stack responds. No host detail (public route) |
| GET | `/health/ready` | public | readiness probe — `200` ready / `503` degraded. Runs `SELECT 1` (1 s cache) and reports a failed boot-time `PRAGMA quick_check`. No host detail |
| GET | `/widgets/cpu` | auth | desktop widget; `usage_percent` is a `/proc/stat` delta sampled per account |
| GET | `/widgets/ram` | auth | |
| GET | `/widgets/network-traffic` | auth | |
| GET | `/widgets/cpu-temp` | auth | |
| GET | `/widgets/update-info` | auth | real check when `UPDATE_CHECK_URL` is set (JSON `{"version":"<semver>"}`, compared with `is_newer_version`, results cached process-wide for 1 h); offline / timeout / malformed payload → `has_app_update=false` without noise. Returns `has_app_update`, `app_current_version`, `app_latest_version`, `server_current_version` — there is **no `has_server_update`** (removed: it was the constant `false`, a measurement never taken) |

## Server control

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| POST | `/server/restart` | admin | `handle_server_restart` — alias of `/power/reboot` over the same primitive |
| POST | `/server/shutdown` | admin | `handle_server_shutdown` — alias of `/power/shutdown` over the same primitive |
| GET | `/server/info` | auth | IP/port/display address |

The two families answer different JSON for different screens but execute the
**same** command (ROADMAP 24.2):

- accepted → `200 {"success":true,"data":{"message":"Server restart initiated","status":"restarting"|"shutting_down"}}`
  for `/server/*`, and `200 {"success":true,"data":{"action":"reboot"|"shutdown"|"suspend"}}`
  for `/power/*`;
- refused by the host (missing sudoers verb, polkit, a systemd failure) →
  `500 {"success":false,"message":"Server restart refused: <host stderr>"}`
  for `/server/*`, and `400 {"success":false,"message":"<host stderr>"}`
  for `/power/*`;
- both stay admin-only. A missing `x-session-token` answers `401` on both
  (the two families used to differ: `401` inline for `/server/*`, `400` from
  `require_admin` for `/power/*` — closed by ROADMAP X.8); an unknown session
  is `401` on both, a non-admin `403`.

The argv is always `sudo systemctl reboot|poweroff|suspend`; the sudoers grant
names exactly those verbs (`src/setup.rs::power_grants`).

## Users — `users/*` (most `admin`)

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/users` | admin | `handle_users_list` — every account carries `is_host_account` (`true` = mirrored Linux/PAM account, not HomeVault's to delete) |
| POST | `/users` | admin | `handle_user_create` |
| GET | `/users/{username}` | admin/self | `handle_user_get` — includes `is_host_account` |
| PUT | `/users/{username}` | admin/self | `handle_user_update` |
| PUT | `/users/{username}/full` | admin | `handle_user_update_full` |
| DELETE | `/users/{username}` | admin | `handle_user_delete` — deletes the Linux account first (`userdel`, retryable on failure), then the row; kills PTY sessions, revokes the account's direct share grants (ACL + DB) and purges its SFTP jail. `400` for a PAM-mirrored account whose Linux user still exists (remove it on the host first) |
| PUT | `/users/{username}/password` | admin/self | `handle_user_update_password` — admin-vs-target sets `must_change_password` (owner must change it at first sign-in); self change clears it. A flagged session is limited server-side (`middleware::password_change_guard`) to whoami/logout/its own password change, other calls get `403 password_change_required` |
| GET | `/users/{username}/sessions` | admin | `handle_admin_user_sessions` — active sessions of that user, tokens masked to 8 chars + `…` |
| DELETE | `/users/{username}/sessions` | admin | `handle_admin_revoke_user_sessions` — revoke all of that user's sessions (`{"revoked": n}`) |
| PUT | `/users/{username}/expiry` | admin | `handle_user_set_expiry` — set (`{"expires_at": "<RFC 3339>"}`, must be future) or clear (`null`) the horizon; enforcement in `AuthService::authenticate` + the session sweep |
| PUT | `/users/{username}/role` | admin | `handle_user_update_role` — demotion revokes every privileged group (`homevault_admin`/`homevault_hostadmin`/`sudo`/`wheel`) **before** the write and signs the account out; promotion provisions the Linux account and grants **only** `homevault_admin` (host root is a separate explicit grant) |
| PUT | `/users/{username}/account-manager` | admin | `handle_user_set_account_manager` — `{enabled}` grants/revokes delegated local-account management (migration 083). The manager can manage non-admin users and non-protected groups only; the role, 2FA, sessions and `homevault_admin`/`homevault_hostadmin` stay admin-only. The web desktop remains admins-only; the role is for API/MCP clients today |
| PUT | `/users/{username}/locked` | admin | `handle_user_set_locked` — a **permanent** lock (clears `locked_until`), mirrored to the Linux password (`usermod -L/-U`, best-effort); an automatic lockout instead expires on its own (see `guides/auth.md`) |
| POST | `/users/{username}/2fa/disable` | admin | `handle_admin_disable_2fa` |
| PUT | `/users/{username}/groups` | admin | `handle_user_update_groups` — blank names are rejected, unknown groups → `400 "Group '<x>' does not exist"` |
| GET | `/users/{username}/home` | auth | `handle_user_home_get` — stored config (`enabled`, `disk`, `path`, `size_bytes`) **plus a live `quota` object** (`enforced`, `filesystem`, `mount_point`, `limit_bytes`, `used_bytes`, `reason_code`, `reason`) read back from the filesystem via `repquota`/`zfs get`; `quota` is `null` when the folder is disabled or absent |
| PUT | `/users/{username}/home` | admin | `handle_user_home_put` (`{ enabled, disk, size_bytes, path }`) — applies the limit and verifies it; a limit the kernel refused is stored but reported as not enforced |
| DELETE | `/users/{username}/home` | admin | `handle_user_home_delete` |
| GET | `/users/{username}/avatar.svg` | auth | `handle_user_avatar` (SVG) |
| GET | `/users/count` | admin | `handle_user_count` |
| GET | `/users/{username}/exists` | admin | `handle_user_exists` |
| POST | `/auth/me/avatar` | auth | `handle_update_my_avatar` (self) |

## Groups — `groups/*` (most `admin`)

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/groups` | auth | `handle_groups_list` |
| POST | `/groups` | admin | `handle_group_create` — every member must be an existing HomeVault user (`400 "User '<x>' not found"`), because each one gets a Linux account |
| GET | `/groups/{name}` | auth | `handle_group_get` |
| PUT | `/groups/{name}` | admin | `handle_group_update` — `{ name?, description? }`: rename (migrates the mirror, group grants, quotas and ACLs) and/or set the description |
| DELETE | `/groups/{name}` | admin | `handle_group_delete` — also purges `hv_groups`, transfer quotas, resource grants and `share_permissions` rows keyed on the display name |
| POST | `/groups/{name}/members` | admin | `handle_group_add_member` — member must be an existing HomeVault user |
| DELETE | `/groups/{name}/members/{username}` | admin | `handle_group_remove_member` |
| GET | `/groups/users` | admin | `handle_groups_list_users` |
| GET | `/users/{username}/groups` | admin/self | `handle_user_groups` |

The `homevault_admin` system group (the application-admin group) is hidden from
ordinary group management and cannot be created/deleted through these endpoints
(see `src/groups.rs` `is_admin_group`). Host root uses the separate
`homevault_hostadmin` group and is never granted automatically.

**Home-directory size:** `POST /users` and `PUT /users/{u}/full` also accept
`home_size_bytes: Option<i64>` (`0`/`null` = no limit; negative → 400; larger
than the disk's free space → 400). It applies a real per-user quota on the
home directory — `setquota -u` on ext2/3/4/xfs/jfs/reiserfs, `zfs set
userquota@` on ZFS — through `sudo -n`, and then **verifies it with a
`repquota`/`zfs get` read-back**: a limit the kernel did not accept is stored
but reported as not enforced (with a `reason_code`), never as applied. See
[Users](./guides/users.md#home-directory-size-limit-2026-09-08) and
[Disk Quotas](./guides/quotas.md).

## Storage — `storage/*` (auth)

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/storage/disks` | auth | `handle_disks_list` |
| GET | `/storage/removable` | auth | `handle_removable_disks_list` |
| GET | `/storage/partitions` | auth | `handle_partitions_list` (`lsblk -l`) |
| GET | `/storage/raid/devices` | auth | `handle_raid_available_devices` — candidate physical disks for the RAID creation wizard. **Every** non-system disk is listed; `inUse` is facts-based (partitions, filesystem, mount point, md/ZFS/Btrfs superblock, system disk), in-use rows carry a human `reason`, and every row reports SMART overall health (`smart_status`: `Healthy` / `Failed` / `Unknown`) |
| POST | `/storage/raid` | admin | `handle_raid_create` — creates one virtual disk **as a tracked background job**: `{message, job_id}` immediately, milestones + completion/failure on the progress SSE of `job_id`. Body: `{ name, level (raid0|raid1|raid5|raid6|raid10|raidz3), devices, filesystem (ext4|xfs|btrfs|zfs), spares? (mdadm `--spare-devices`, or a native ZFS `spare` vdev group; Btrfs refuses with an alternative), allow_missing? (degraded create, mdadm redundant levels only) }`. `raidz3` is ZFS-only (four-disk minimum); any other filesystem is a `400` naming the ZFS layout before the device gate. Gates answer 400 synchronously: pure request-shape validation, **server-side device gate** (every device must exist, not host the system disk, and be entirely free) and name-collision pre-check |
| GET | `/storage/virtual-disks` | auth | `handle_virtual_disks_list` — **all** virtual disks (mdadm + ZFS + Btrfs) |
| DELETE | `/storage/virtual-disks/{name}` | admin | `handle_virtual_disk_delete` — dispatches mdadm vs ZFS/Btrfs. mdadm: unmount failures abort the destroy before `mdadm --stop`/`--zero-superblock` |
| POST | `/storage/virtual-disks/{name}/destroy` | admin | `handle_virtual_disk_destroy` — password-confirmed destroy `{confirm_name, password}`; the typed name must match and the admin's own password is re-verified before the wipe (DSM parity). The UI path; `DELETE` stays for API clients |
| POST | `/storage/virtual-disks/{name}/scrub` | admin | `handle_virtual_disk_scrub` — ZFS/Btrfs scrub, mdadm `sudo mdadm --action check` |
| GET | `/storage/virtual-disks/{name}/scrub` | auth | `handle_virtual_disk_scrub_status` — current scrub state + `in_progress` |
| GET | `/storage/importable-pools` | auth | `handle_importable_pools_list` — foreign ZFS/Btrfs pools on attached disks |
| POST | `/storage/import-pool` | admin | `handle_import_pool` — `{ name, pool_type, new_name? }`, ZFS `zpool import` / Btrfs mount |
| GET | `/storage/raid` | auth | `handle_raid_arrays_list` — legacy; Storage's Virtual-disks tab is preferred |
| POST | `/storage/raid/{name}/rebuild` | admin | `handle_raid_rebuild` — reused by Storage's Rebuild button; the optional `replacement_device` passes the same free-device gate as creation |
| POST | `/storage/virtual-disks/{name}/grow` | admin | `handle_virtual_disk_grow` — mdadm only: `{ device }` (a free whole disk) → `mdadm --add` + `--grow --raid-devices N+1`; the reshape runs in the background (raid0/raid10 and non-mdadm technologies refused) |
| GET | `/storage/virtual-disks/{name}/level-options` | auth | `handle_virtual_disk_level_options` — mdadm only: `{ current_level, member_count, reshape_in_progress, options[{ level, allowed, reason? }] }`; every refusal names its reason, all targets are listed |
| POST | `/storage/virtual-disks/{name}/migrate-level` | admin | `handle_virtual_disk_migrate_level` — mdadm only: `{ target_level (raid0|raid1|raid5|raid6|raid10), backup_confirmed (must be true) }` → `mdadm --grow --level N --backup-file <{DATA_DIRECTORY}/raid-backups/…>`; only the documented transitions (0→5/10, 1→5, 5↔6), target minimum member count enforced, an already-running reshape refused; the reshape runs in the background. Missing `mdadm` → `503` |
| GET | `/storage/virtual-disks/{name}/btrfs-layout` | auth | `handle_btrfs_layout_status` — Btrfs only: `{ pool, mount_point, layout (legacy|at), layout_name, rollback_available, migration_required }` |
| POST | `/storage/virtual-disks/{name}/btrfs-layout` | admin | `handle_btrfs_layout_migrate` — explicit legacy → `@`/`@snapshots`: moves `.snapshots` to `@snapshots`, data into `@`, rewrites both fstab entries, remounts; serialized, failure restores the moved entries. Already migrated → `503`/`400` named |
| POST | `/storage/virtual-disks/{name}/btrfs-rollback` | admin | `handle_btrfs_rollback` — `{ snapshot, confirm: true }`; legacy layout → `503` (`json_unavailable`) naming the migration; otherwise a `pre-rollback-*` safety snapshot, `@` swapped for the snapshot, previous `@before-rollback-*` kept, both subvolumes remounted; recovery path run on failure |
| GET | `/storage/available-home-disks` | auth | `handle_available_home_disks` (valid session, not admin) |
| GET | `/storage/disks/{name}/smart/capabilities` | auth | `handle_disk_smart_capabilities` — whether `smartctl` supports this disk |
| GET | `/storage/disks/{name}/smart/test` | auth | `handle_disk_smart_test_status` — running self-test status |
| POST | `/storage/disks/{name}/smart/test` | admin | `handle_disk_smart_test_start` — start a SMART self-test (`test_type` is whitelisted to `short`/`long`/`conveyance`); logs the admin actor to the event log |

> The former `/storage/pools*` endpoints (`handle_pools_list`,
> `handle_advanced_pool_create`, `handle_pool_get`, …) were **removed** in the
> 2026-09-07 RAID↔Storage re-scope: Storage manages every virtual disk via
> `virtual-disks` above, RAID only creates. `src/pools.rs` (`PoolService`) is
> still reached through the virtual-disk and cache paths.

## Disk Encryption — `encryption/*`

LUKS2 encryption for a single available (unassigned) block device — not RAID
arrays or ZFS pools. Unlock is always manual: no key material is ever stored on
the server, so a volume stays locked after every reboot until an admin re-enters
the passphrase through this API. See [guides/encryption.md](./guides/encryption.md).

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/encryption/volumes` | auth | `handle_encryption_list` (includes live locked/unlocked status, probed concurrently) |
| GET | `/encryption/available-devices` | auth | `handle_encryption_available_devices` |
| POST | `/encryption/volumes` | admin | `handle_encryption_create` (`{ device, filesystem, label, passphrase }`) → **`{ job_id }`**, the work runs in a tracked job (`encryption_create`); validation failures still answer 400 synchronously |
| POST | `/encryption/volumes/{id}/unlock` | admin | `handle_encryption_unlock` (`{ passphrase }`; a failed mount rolls the mapper back) |
| POST | `/encryption/volumes/{id}/lock` | admin | `handle_encryption_lock` (a failed unmount is reported, not ignored) |
| POST | `/encryption/volumes/{id}/passphrase` | admin | `handle_encryption_change_passphrase` (`{ old_passphrase, new_passphrase }`, new one must clear the policy and differ) |
| DELETE | `/encryption/volumes/{id}` | admin | `handle_encryption_delete` (`{ confirm: true }`, erases LUKS key slots — **refused** unless the volume is verified unmounted and closed; answers `data.warnings[]` for cleanup problems) |

**Passphrase policy** for new passphrases (create + rotation): at least 12
characters, at least four distinct ones, not a wordlist entry, not the volume
label. Unlock is exempt so pre-existing volumes stay usable.

**Deletion is gated** (ROADMAP 9.2): `umount` → `cryptsetup close` → re-verify
`/proc/self/mounts` and `cryptsetup status` → `cryptsetup erase` → `wipefs -a`.
Any refusal answers 400 and keeps the metadata row.

## Explorer — `explorer/*` (auth)

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/explorer/list` | auth | `?path=`; response has `truncated: bool` (cap `MAX_DIR_ENTRIES = 10_000`), `permission` (`deny`\|`read`\|`write`\|`full`) and `restrictions` (`disable_browsing`/`disable_modification`/`disable_download`). Listing is refused when the share disables browsing |
| GET | `/explorer/roots` | auth | `handle_explorer_roots` — labelled sidebar roots (`{path, kind, name, permission}`): the session user's home, each PathGuard root, then every mounted virtual disk (`kind:"virtualdisk"`). A member only sees their home and the grants they hold |
| GET | `/explorer/search` | auth | `?path=&query=&max_results=&fuzzy=` — refused when the share disables browsing |
| GET | `/explorer/disks` | auth | `handle_explorer_disks` — mounted volumes grouped `system\|internal\|raid\|external\|cloud\|network`; per entry `mount_point`/`filesystem`/`size_bytes`/`used_bytes` + optional `label`/`vendor`/`model`/`serial`. A member only sees mounts that are their roots or sit inside one |
| POST | `/explorer/mkdir` | auth | `?path=` — requires `write` |
| POST | `/explorer/delete` | **admin** | `?path=` — permanent; internal use only now, see `/explorer/trash`. Even for admins the path passes the PathGuard, and deleting a browsing root itself is refused. Since 2026-10-30 a path outside the roots is allowed **only** inside the scratch space (`$TMPDIR`/`/tmp`/`/var/tmp`) |
| POST | `/explorer/rename` | auth | `?path=&new_name=` — **both** paths are path-guard validated and require `write` |
| POST | `/explorer/save` | auth | `handle_explorer_save` — inline text/code editor, atomic temp-file + rename, 5 MB cap; requires `write` |
| POST | `/explorer/copy` | auth | `?source=&destination=&strategy=overwrite\|keep_both\|skip`; requires `read` on the source and `write` at the destination (a read-only share can be copied out of). Directories return `{job_id, sse_url}`; a single file answers `{message, method, instant, bytes}` where `method` is `clone` \| `kernel_copy` \| `stream` (a clone shares the source's blocks — see *Instant copy* below); every non-clone copy is SHA-256 verified end-to-end |
| POST | `/explorer/move` | auth | `?source=&destination=&strategy=`; same job pattern; source deleted only after a verified copy. Answers `{message, method, instant}` — `method` is `null` for a same-filesystem atomic rename (nothing was copied) and names the strategy only for the cross-filesystem fallback. Requires `write` on both sides |
| GET | `/explorer/instant-copy` | auth | `handle_explorer_instant_copy_get` — `{ mode: "auto"\|"off", mounts: [{ mount_point, fs_type, reflink }] }`; the mounts are evidence for the UI, never a gate |
| PUT | `/explorer/instant-copy` | **admin** | `handle_explorer_instant_copy_update` — body `{ mode }`; strict parse, an unknown value is refused rather than coerced; persisted in the singleton `instant_copy_policy` row (migration 062) |
| GET | `/shares/{name}/{share_type}/quota` | **admin** | per-share size ceiling: `backend` (`zfs`/`btrfs`/`xfs_project`/`unsupported`), `supported`, `filesystem`, `reason` (when unsupported), `quota_bytes`, `used_bytes`, `free_bytes`, `percent`, `quota_human`, `used_human` |
| PUT | `/shares/{name}/{share_type}/quota` | **admin** | `{ quota_bytes }` (`0` removes it); the filesystem is changed **first**, and the row only records what it accepted; `400` under 1 MiB or over 1 PiB, or when the filesystem cannot enforce a limit |
| DELETE | `/shares/{name}/{share_type}/quota` | **admin** | remove the ceiling (filesystem, then row) |
| GET/POST | `/explorer/permissions` | auth | `?path=&mode=` — read/set POSIX mode. **Changing** the mode requires `full` on the folder (DSM's "Change permissions" right), because `chmod` rewrites the ACL mask SMB/WebDAV see |
| GET | `/explorer/preview` | auth | text/image preview; `previewable: false` for non-text binaries; refused when the share sets `disable_download` |
| POST | `/explorer/extract` | auth | `?path=&destination=` — members listed + validated pre-flight (no absolute names, no `..`, no symlink members), extracts into a staging dir verified confined below the destination, `.gz` decompresses straight to disk |
| POST | `/explorer/compress` | auth | JSON body `{ paths: string[], destination, format? }` — `--` separator before the member names (no argv injection) |
| GET | `/explorer/download` | auth \| ticket | raw file download; `?ticket=` (single-use, from `/explorer/download-ticket`) *or* `x-session-token` header — **never** the session token in a URL; HTTP `Range` (206), `X-Content-Sha256` digest header for files ≤ 64 MB; 429 if the transfer quota is exhausted, 503 if the quota guard itself cannot answer (fail closed). Refused when the share sets `disable_download` |
| POST | `/explorer/download-ticket` | auth | `?path=` — mint a single-use 60 s download ticket (checks path + download quota) |
| GET | `/explorer/thumbnail` | auth \| ticket | image thumbnail; `?ticket=` (multi-use, directory-scoped) or session header; refused when the share sets `disable_download` |
| POST | `/explorer/thumbnail-ticket` | auth | `?path=` — mint a multi-use 5 min thumbnail ticket for a directory; refused when the share sets `disable_download` |
| POST | `/explorer/upload` | auth | multipart `file` + `?path=` + `?strategy=`; `fsync`'d before success; SHA-256 in `checksums[]`, `skipped[]` for `strategy=skip`; optional `X-Expected-Sha256` header (single-file) |
| POST | `/explorer/upload/session` | auth | `?path=&filename=&size=&sha256?=` — open a resumable upload session; owned by the calling user; capped per user (20) and globally (256) **counted in `upload_sessions`**, so a restart does not reset them; 429 beyond, 429 over quota, 503 if the quota guard cannot answer. Answers `{ session_id, offset, expires_in_seconds }`; the row survives a restart (migration 074) |
| DELETE | `/explorer/upload/session` | auth | `?session_id=` — abort a resumable upload, remove the partial temp file; 403 unless the session belongs to the caller |
| POST | `/explorer/upload/chunk` | auth | **streamed** body + `?session_id=&offset=` — write one chunk at the server's authoritative offset (409 + current offset on mismatch); the body is written frame by frame and capped at 32 MiB and at the session's remaining size, so an over-cap chunk is `413` with the offset to resume at and commits nothing; 403 unless the session belongs to the caller. Answers `{ offset, bytes_total, speed_bps }`. The session is rehydrated from `upload_sessions` on a cache miss, so a chunk sent after a restart resumes at the stored offset |
| POST | `/explorer/upload/complete` | auth | `?session_id=` — fsync + SHA-256 + atomic rename into place, then debit the quota; 403 unless the session belongs to the caller |
| POST | `/explorer/checksum` | auth | JSON body `{ path, algorithm?, compare_with?, async? }` — on-demand digest (`sha256` default, `sha512`, `blake3`; MD5 and unknown values are `400`). A file ≤ 8 MiB answers `{ path, algorithm, checksum, file_size, duration_ms }` inline; a larger one (or `async: true`) answers `{ job_id, sse_url, status }` and the job reports bytes/speed/ETA like any transfer. `compare_with` adds `{ compare_with: { path, checksum, file_size }, match }`. Both paths pass the PathGuard |
| GET | `/explorer/checksum/result` | auth | `?job_id=` — the outcome of an asynchronous checksum (`{ state, path, algorithm, checksum, file_size, duration_ms, match, error }`), owner or admin only, 15-minute lifetime |
| GET | `/explorer/transfers/active` | auth | live transfers for the tray: `{ uploads: [{ session_id, filename, dir, bytes_received, bytes_total, progress, speed_bps, eta_seconds, state, created_at, last_chunk_at }], transfers: [{ id, zone, kind, state, label, progress, bytes_processed, bytes_total, speed_bps, eta_seconds, method, detail, cancel_path }], counts: { uploads, downloads, copies, total } }`. Uploads come from `upload_sessions` (a row, not a job), jobs from the progress manager scoped to the caller; `zone` is `upload`\|`download`\|`copy`; `cancel_path` is API-relative (`/progress/{id}`) and `null` for an upload (abort it with the session id) |
| GET | `/explorer/transfers/history` | auth | `?limit=&offset=&operation=&status=&since=` — finished transfers, newest first: `{ entries: [{ id, job_id, username, operation, source_path, dest_path, bytes_total, duration_ms, method, verified, status, error_message, created_at, completed_at }], has_more, limit, offset }`. A non-admin sees only their own rows, an admin sees every user's; unknown `operation`/`status` values are `400`; retention 90 days |
| POST/GET/DELETE | `/explorer/trash` | auth | `?path=` (POST) — move to trash / list trash / empty trash |
| POST | `/explorer/trash/restore` | auth | `?id=` (cross-device EXDEV fallback) |
| DELETE | `/explorer/trash/{id}` | auth | permanently delete one trashed item |
| GET | `/explorer/trash/policy` | auth | current `{ retention_days, max_bytes, last_purge_at }` (default 30 days / 10 GiB, `0` = that limit off) |
| PUT | `/explorer/trash/policy` | **admin** | body `{ retention_days, max_bytes }`; `0` is a real "off" value, not a reset to the default |
| POST | `/explorer/trash/purge-now` | **admin** | run the retention pass now with the saved policy, answers `{ purged, freed_bytes }` |
| POST | `/explorer/format` | admin | `{ device, fstype, label? }` — format a block device as `ext4` or `exfat` (`mkfs.*` under `sudo`); `fstype` allow-list and the device validated against the available-disk list **before** any `mkfs` runs |
| GET | `/explorer/history` | auth | `?path=` — **Rewind**: the snapshot timeline covering that file/folder, oldest→newest, each with a read-only `snapshot_path` under `.zfs/snapshot/` (ZFS) or `.snapshots/<share>/<name>/` (Btrfs); empty when neither backend covers the path |
| POST | `/explorer/watch/ticket` | auth | JSON body `{ path }` — validates the path against the caller's roots, mints a single-use 15 s ticket **bound to (user, path)** for the filesystem-watcher SSE connection (EventSource cannot set custom headers) |
| GET | `/explorer/watch` | auth \| ticket | `?ticket=&path=` — SSE stream; the ticket only opens the **exact path it was minted for** (otherwise refused), `change` events are filtered component-based to the watched directory + `heartbeat` every 30 s; the inotify watch is released when the stream drops |
| GET | `/explorer/snapshots` | auth | `?path=` — lists ZFS snapshots covering that path with a `browse_path` under `.zfs/snapshot/…` (read-only browsing) |
| GET | `/remote-mounts` | admin | configured **network locations** (CIFS/NFS remote mounts): `{ available, kinds, root, mounts: [{ name, kind, server, source, username, domain, options, read_only, enabled, created_at, last_error, target, mounted, has_credentials }] }`. The password is never serialised — only `has_credentials` |
| POST | `/remote-mounts` | admin | `{ name, kind?, server, source, username?, password?, domain?, options?, read_only?, enabled?, mount_now? }` — adds one; `name` is a lowercase slug and the mount point is derived from it (`/mnt/homevault-remote/<name>`), never from the body. A helper-less host is a `503` naming `mount.cifs`/`mount.nfs` |
| PUT | `/remote-mounts/{name}` | admin | edit one (blank `password` keeps the stored one); a mounted location is unmounted, re-rendered and mounted again, so an option or host change never leaves stale data |
| DELETE | `/remote-mounts/{name}` | admin | unmount (when up), delete the credentials file, forget it |
| POST | `/remote-mounts/{name}/mount` \| `/unmount` | admin | mount / unmount one (both idempotent); a failure carries the tool's stderr in `last_error` |
| POST | `/remote-mounts/reconcile` | admin | mount every `enabled` location that is not up yet — the same pass the server runs at startup; answers `{ failed: [{ name, error }], mounts }` |
| POST | `/explorer/eject` | admin | `?path=<mount>` — safe-eject a removable drive (guarded to removable block devices only) |
| GET | `/explorer/shares` | auth | list the caller's "share by link" links — `protected` (a password is set), `download_count`, `max_downloads` and `exhausted` (the cap is spent); the Argon2 hash and the in-flight reservation are never serialised |
| POST | `/explorer/shares` | auth | `{ path, mode?, expires_days?, password?, max_downloads?, allowed_users?, allowed_groups? }` — create a download link to a **file or folder** (`mode`: `anyone` \| `signed_in`; default 30-day expiry, `0` = never). A folder is served as a tar.gz snapshot built at creation. `password` is 8–256 characters, stored as an Argon2id hash; `max_downloads` is 1–100000 and **only counts completed transfers**, so `1` is the one-time link. `allowed_users`/`allowed_groups` (comma-separated, max 32) target a `signed_in` link; empty = any signed-in account. Creation requires `read` on the path and is refused when the share disables downloads |
| DELETE | `/explorer/shares/{token}` | auth | revoke a link (owner or admin); any unlock minted for it stops working immediately, and a folder link's archive is removed |
| GET/POST | `/explorer/file-requests` | auth | list the caller's public upload links / `{ path, message?, expires_days?, password?, max_uploads? }` — creates an upload link for a **folder** (requires `write`). The password is 8–256 characters (Argon2id), `max_uploads` 1–100000 counts only completed files |
| DELETE | `/explorer/file-requests/{token}` | auth | revoke an upload link (owner or admin) |
| GET | `/request/{token}` | public | public upload page (plain HTML: optional password field, then a multi-file input). Expired/spent/unknown → `404`; spent → `429` |
| POST | `/request/{token}/upload` | public | multipart upload; the `password` field must precede the `file` field (a file first is refused without writing). Max 2 GiB per file, 4 concurrent uploads, collisions keep both. The owner's `write` grant and Explorer app are re-checked before every upload |
| POST | `/share/{token}/unlock` | public | `{ password }` → `200` with `Set-Cookie: hv_share_unlock=…` (HttpOnly, SameSite=Lax, `Secure` over TLS, scoped to `Path=/share/{token}`, 15 min). `401` carries `x-hv-share-auth: password`; `429` once 8 wrong passwords were tried inside 5 minutes. A wrong password or an oversized body is refused before Argon2 runs |
| GET / HEAD | `/share/{token}` | public | **public** download (outside `/api`, before the SPA fallback). A password-protected link accepts either the unlock cookie or an `x-share-password` header — **never** the password in the query string. `signed_in` links accept the `x-session-token` header **or** the `hv_session` login cookie (a navigation cannot send a header), and a `401` carries `x-hv-share-auth: session`. HTTP `Range` (206) + `X-Content-Sha256` for files ≤ 64 MB; per-token bandwidth bucket + global concurrency cap; `Cache-Control: no-store`. `HEAD` spends no download slot, and an expired, revoked or spent link answers `400` (indistinguishable, so the endpoint is not a token oracle). A browser navigation (`Accept: text/html`) that cannot be answered with the file itself receives the SPA page instead of JSON — the password form, or the reason the link is unavailable |

Paths are guarded by `config::PathGuard` (`AppState.path_guard`). See
[File Explorer](./guides/explorer.md) for the trash/compress/progress-job
design and everything found+fixed during live verification.

**Authorization (P0 pass, 2026-11-08).** `PathGuard` proves reachability, not a
grant level. Every mutating Explorer route now also checks the effective
`resource_shares` level: `write` for create/upload/rename/trash/compress/
extract/save, `write` on both sides for a move, `read` on the source + `write`
on the destination for a copy, and `full` for `chmod`. `preview`, `thumbnail`
and `thumbnail-ticket` honour `disable_download`; `list` honours
`disable_browsing`. The listing/roots responses carry the level so the UI can
disable the same actions. A member whose `app:explorer` grant is `deny` is
refused the whole `/api/explorer/*` surface by
`middleware::explorer_access_middleware` — DSM's "Application Privileges".

### Transfer integrity

Copy, move (cross-filesystem fallback) and upload guarantee the stored bytes
match the source:

- **Copy/move** (`FileBrowser::copy_file_instant` in `src/browser.rs`, built on
  `src/browser/reflink.rs`) tries a copy-on-write clone (`ioctl(FICLONE)`), then
  a kernel-side `copy_file_range`, and only then the verified stream. The
  stream computes the source's SHA-256 while copying, `fsync`s the destination,
  then reads it back and compares hashes; on a mismatch the corrupt destination
  is deleted and `BrowserError::IntegrityMismatch` is returned — so a move never
  deletes the source unless a byte-identical copy is confirmed on disk. A clone
  is byte-identical by construction and skips that read-back (hashing it would
  re-read the whole file and cancel the speed-up); it is only claimed when the
  ioctl actually succeeded. Symlinks are recreated as symlinks (never
  dereferenced), and permission bits, ownership (uid/gid) and mtime are
  preserved (best-effort, like `rsync -a`).
- **Upload** (`handle_explorer_upload`) `flush`es + `fsync`s each file before
  reporting success, returns each file's SHA-256 in `checksums[]`, and enforces
  an optional client `X-Expected-Sha256` header.
- **Cache mover** (`src/cache.rs`) uses `rsync -a`, which verifies every block
  and only removes source files it successfully transferred; rsync exit code 24
  ("vanished source files") is treated as success rather than failing the run.

## Shares — `shares/*` (all `admin`)

Every Shares endpoint (list, get, config, service control, and ACL
permissions) requires an **admin** session. A past audit found `handle_shares_list`
and `handle_share_get` accepted any non-empty `X-Session-Token`; they now use
`require_admin` (`src/handlers/shares.rs`).

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/shares` | admin | `handle_shares_list` |
| POST | `/shares` | admin | `handle_share_create` |
| GET | `/shares/{name}/{share_type}` | admin | `handle_share_get` |
| PUT | `/shares/{name}/{share_type}` | admin | `handle_share_update` |
| PUT | `/shares/{name}/{share_type}/toggle/{enabled}` | admin | `handle_share_toggle` |
| DELETE | `/shares/{name}/{share_type}` | admin | `handle_share_delete` |
| GET | `/shares/{name}/{share_type}/snapshots` | auth | `handle_share_snapshots_list` — snapshots covering the share (ZFS dataset / Btrfs subvolume) with `browse_path`; `{ share, pool, snapshots[] }` |
| POST | `/shares/{name}/{share_type}/snapshots` | admin | `handle_share_snapshot_create` — `{ name? }`; creates a recursive snapshot (`manual-YYYYMMDD-HHMMSS` by default), returns it with `browse_path` |
| DELETE | `/shares/{name}/{share_type}/snapshots/{snapshot}` | admin | `handle_share_snapshot_delete` — deletes only that share backend's snapshot; unsafe names `400` |
| POST | `/shares/{name}/{share_type}/snapshots/{snapshot}/restore` | admin | `handle_share_snapshot_restore` — `{ path (relative, no dot-dot or absolute path), confirm: true }`; restores a file/folder from the snapshot into the live share, atomic file copy, folder merge; `403` outside the share, unsupported Btrfs non-subvolume `503` |
| GET/PUT | `/shares/smb\|nfs\|ftp\|webdav\|sftp/config` | admin | service config |
| GET | `/shares/ws-discovery` | admin | WS-Discovery daemon state (`available`, `enabled`, `running`, `detail`). The `ws_discovery` toggle itself travels in the SMB config payload; discovery only, no share is exposed |
| GET | `/shares/sftp/users` | admin | users with SFTP access + key status (separate `homevault-sftp.service`, see [Shares guide](./guides/shares.md)) |
| PUT | `/shares/sftp/users/key` | admin | set a user's SFTP public key `{ username, public_key }` |
| POST | `/shares/sftp/rebuild` | admin | reconcile SFTP jails against current ACL grants |
| POST | `/shares/{share_type}/start\|stop\|restart` | admin | service control |
| GET/POST/DELETE | `/shares/{name}/{share_type}/permissions` | admin | per-user/group ACL |
| GET | `/shares/smb/time-machine` | admin | `handle_smb_time_machine_list` — per-share `{ enabled, quota_gb }` |
| PUT | `/shares/{name}/smb/time-machine` | admin | `handle_smb_time_machine_set` — `{ enabled, quota_gb }` (`0` = unlimited), renders the one managed `# homevault-smb-vfs` block |
| GET | `/shares/rsync` | admin | `handle_rsync_state` — settings, modules, `share_candidates`, `users_with_secret` (**never** a password), service state, limits |
| PUT | `/shares/rsync/config` | admin | `handle_rsync_settings_update` — `{ enabled, port?, max_connections?, chroot? }` |
| POST | `/shares/rsync/modules` | admin | `handle_rsync_module_create` — `{ name, path, comment?, read_only?, users[], enabled }` |
| PUT/DELETE | `/shares/rsync/modules/{name}` | admin | update (toggle / read-only / users) or remove a module |
| PUT/DELETE | `/shares/rsync/users/{user}/secret` | admin | `handle_rsync_secret_set` — sets an rsync password (write-only) / forgets it |
| POST | `/shares/{name}/{share_type}/permissions/sync` | admin | re-apply ACLs |
| GET | `/shares/definitions` | admin | logical shares: one row per `(name, path)`, `protocols[]`, `conflicts[]` |
| POST | `/shares/definitions` | admin | `{name, path, comment?, protocols[], read_only?, guest_ok?, restrictions?}` — creates every binding, rolls back on failure |
| POST | `/shares/{name}/bind` | admin | add one protocol binding to an existing logical share (same path) |
| DELETE | `/shares/{name}/bind/{share_type}` | admin | remove one protocol binding |
| GET/POST/DELETE | `/shares/{name}/permissions` | admin | one grant applied to every binding of the logical share |
| POST | `/shares/{name}/permissions/sync` | admin | re-apply the stored ACLs on every binding |
| GET | `/shares/permissions` | admin | flat permission matrix |
| GET | `/shares/permissions/report` | admin | CSV report (`text/csv`) |
| GET/PUT | `/shares/{name}/{share_type}/acl` | admin | live POSIX access/default ACL, `{entries, default_entries, recursive}` |
| GET/PUT | `/shares/{name}/nfs/rules` | admin | per-client NFS rules `[{client, access, options}]` |
| GET/PUT | `/shares/smb/previous-versions` and `/shares/{name}/smb/previous-versions` | admin | `shadow_copy2` per SMB share |
| POST | `/shares/smb/domain/join` | admin | `{username, password}` — joins the configured AD realm (password on stdin) |
| POST | `/shares/smb/domain/leave` | admin | leaves the current AD domain |
| GET | `/users/{username}/permissions` | admin | `handle_user_permissions` |
| GET | `/users/{username}/permissions/effective` | admin/self | direct + group grants (Permission Inspector equivalent) |
| GET | `/shares/access` | admin | `handle_list_access` — every per-member resource grant (optional `?member=`) |
| GET | `/shares/access/mine` | auth | `handle_my_access` — the caller's own effective access (`all: true` for admins) |
| POST | `/shares/access` | admin | `handle_grant_access` — `{member, kind, resource, permission?}` |
| DELETE | `/shares/access` | admin | `handle_revoke_access` — `?member=&kind=&resource=` |
| GET | `/shares/access/groups` | admin | `handle_list_group_access` — every group-level grant (optional `?group=`) |
| POST | `/shares/access/groups` | admin | `handle_grant_group_access` — `{group, kind, resource, permission?}` |
| DELETE | `/shares/access/groups` | admin | `handle_revoke_group_access` — `?group=&kind=&resource=` |

`/shares/access` is the **fine-grained per-member** layer (apps / folders /
shares), separate from the POSIX-ACL share permissions above. `kind` ∈ {`app`,
`folder`, `share`}, `permission` ∈ {`deny`, `read`, `write`, `full`} — `deny`
wins over every allow. See
[Users](./guides/users.md) for the server-side filter contract.

`share_type` ∈ {`smb`, `nfs`, `ftp`, `webdav`, `sftp`}. Permission grant applies the
real POSIX ACL **before** the DB write; revoke deletes the DB row even if the ACL
removal fails (see `src/permissions.rs`).

**Validation (2026-10-21).** Share values are embedded verbatim in root-owned
config files, so they are validated before any privileged write and a rejection
is a `400` naming the field:

- `name`: ASCII letters/digits/`-`/`_`/`.` (≤ 64), no leading `.`, no `..`, not
  a Samba-reserved section (`global`, `homes`, `printers`, `admin$`).
- `comment`: single line (no control characters), ≤ 200 chars.
- `path`: absolute, no whitespace/control/`( ) , # ;`, no `..`.
- `options` (NFS): whitespace-free tokens, no `( ) , # " '`, ≤ 16 entries —
  they are written as `path *(opt1,opt2)`.
- SMB global config: one-line `workgroup` (≤ 15), `security_mode` ∈
  {`user`,`share`,`domain`,`server`,`ads`}, octal `create_mask`/`directory_mask`.
- FTP config: single-line `welcome_message`, non-zero ports, `passive_ports` =
  `min` or `min-max` with min < max, `max_clients` ≥ 1.

`PUT /shares/{name}/nfs/toggle/{enabled}` rewrites the export line
(`# homevault-nfs-disabled:<name>` tag) and reloads `exportfs -ra`; the state
survives a reboot. A share name matching several exports in `/etc/exports`
answers `400` listing the colliding paths instead of silently editing all of
them. All share operations run on the blocking pool under a timeout
(`spawn_blocking` + 30 s, SFTP jail rebuild 120 s) — a hung
`systemctl`/`mount` returns `{ success:false, message }`, never an indefinitely
pending request.

## Network — `/api/network/*` (router in `src/handlers/network.rs`)

Read endpoints `auth`; config/state/WiFi mutations `admin`.

| Method | Path | Auth |
|--------|------|------|
| GET | `/network/interfaces` | auth |
| GET | `/network/interfaces/{name}` | auth |
| GET | `/network/interfaces/{name}/config` | auth |
| PUT | `/network/interfaces/{name}/config` | admin — applies live, persists (netplan/ifupdown), then `netplan apply` + wire verify; a failed reconcile rolls everything back and answers `{ success:false, message }` |
| PUT | `/network/interfaces/{name}/state` | admin |
| GET | `/network/wifi/scan` | auth |
| GET | `/network/wifi/status` | auth |
| POST | `/network/wifi/connect` | admin |
| POST | `/network/wifi/disconnect` | admin |
| GET | `/network/diagnostics` | auth |
| GET | `/network/summary` | auth |
| GET | `/network/public-ip` | auth |
| GET | `/network/services` | auth | active network services |
| GET | `/network/bonds` | auth | `{ bonds[], candidates[], modes[] }` — `candidates` are **physical** NICs only (a Docker/veth/`vmnet` device is `ARPHRD_ETHER` but has no port behind it); each mode carries `needs_switch_configuration` |
| POST | `/network/bonds` | admin | `{ name?, mode, members[] }` — `mode` is `802.3ad` or `active-backup` (a driver mode the appliance does not offer is a `400` naming the two); members must be distinct physical NICs. Creates the bond, or attaches the missing members to a live one. **`400` before any `ip` runs** when the caller reached the appliance through an interface the change would re-role (peer from the TCP socket, never a header). Answers `(persisted)` / `(temporary — persistence unavailable)` |
| DELETE | `/network/bonds/{name}` | admin | deletes the bond, detaches its members back to ordinary NICs and removes its persisted file; refusals are the same session rule plus "not a bond" |
| GET | `/network/vlans` | auth | `{ vlans[], parents[], max_vid: 4094 }` — parents are physical NICs and bonds |
| POST | `/network/vlans` | admin | `{ parent, vid }` → creates `<parent>.<vid>`; `vid` is bounded to `1..=4094` (0 and 4095 are reserved) |
| DELETE | `/network/vlans/{name}` | admin | deletes the VLAN and its persisted file |
| GET | `/network/routes` | auth | `{ available, routes[] }` — saved static routes (`destination`, `via`, `metric`, `interface`, `active`); `available: false` when `ip` is missing |
| POST | `/network/routes` | admin | `{ destination, via, metric?, interface }` — CIDR with non-zero prefix, same-family unicast next hop, name ≤ 15 chars, metric 0–65535; `sudo ip route replace` then read-back then JSON save; one route per destination; `400` malformed, `503` missing `ip` |
| DELETE | `/network/routes` | admin | `{ destination, interface }` — removes the kernel route and the saved entry; an unknown pair is `400` |

## mDNS / Zeroconf — `mdns/*`

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/mdns/status` | auth | `{ enabled, available, running, host_name, fqdn, http_url, services[] }` — the `homevault.local` address + advertised share types. `enabled` is the **live** flag (the admin's Settings choice), not the stored one. Publishing itself is automatic (`src/mdns.rs` reconciler, skipped while disabled); see [guides/mdns.md](./guides/mdns.md). |

## Chat (AI Assistant) — `chat/*`

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| POST | `/chat/send` | auth | chat with local Ollama model |
| POST | `/chat/send/stream` | auth | streaming chat — SSE (RAG-enriched prompt, see [Chat guide](./guides/chat.md)) |
| GET | `/chat/models` | auth | list installed models |
| POST | `/chat/models/pull` | admin | pull a model |
| GET | `/chat/models/recommend` | auth | recommendations — the 2026-07-19 docs audit found this handler took no session check; it now takes `HeaderMap` like the rest of the file and answers 200 + `{"success":false}` when unauthenticated (the `/chat/send` convention). |
| GET | `/chat/model` | auth | current model |
| PUT | `/chat/model` | admin | set current model |

The system prompt is built per the request `language` (defaults to English); replies
respect the user's UI language. When the RAG knowledge base is indexed, the system
prompt is automatically enriched with relevant context chunks; a RAG failure
degrades to the plain prompt and is logged (it never fails the chat).

## RAG Knowledge Base — `rag/*`

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/rag/status` | auth | index status: last indexed timestamp (only advances on a completed run), doc/chunk counts (only replaced by a completed run), `embedding_model` (always the model the index needs), `status` (`idle`/`indexing`/`error`) |
| POST | `/rag/index` | admin | full background reindex **and reconcile** (catalog + features + i18n); answers `{"success":false}` when a reindex is already running |
| POST | `/rag/search` | admin | semantic search test — `{ query, top_k }` → results + context string; an embedding error is returned as `{"success":false, "error":…}`, never as an empty result |
| GET | `/rag/documents` | admin | list all indexed documents (id, source, title, language); a DB error is `success:false`, never an empty list |

Embedding model is `nomic-embed-text` (hardcoded). Admin must pull it before
indexing (Settings → AI Assistant → Knowledge Base). Embeddings stored as f32
BLOBs; similarity search is cosine in Rust. A reindex removes documents a
source no longer produces, and an embedding failure fails the run (`status =
error`) instead of storing chunks a search can never match.

## External Agents (MCP) — `mcp/*` + `POST /mcp`

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/mcp/status` | admin | kill-switch state, global permissions, token prefixes (never secrets) |
| PUT | `/mcp/status` | admin | `{enabled}` — disabling keeps tokens/permissions |
| PUT | `/mcp/permissions` | admin | replaces the global permission set (everything defaults to denied) |
| POST | `/mcp/tokens` | admin | mints `hv_mcp_…` (cap 20); full value returned exactly once |
| DELETE | `/mcp/tokens/{id}` | admin | revoke |
| POST | `/mcp` (no `/api` prefix) | Bearer `hv_mcp_…` | MCP Streamable HTTP: `initialize`, `tools/list`, `tools/call`, `ping` |

Disabled/invalid/revoked tokens all answer the same 401. The tool set is
filtered per request from the permission snapshot (system info, apps,
catalog, files, VMs, knowledge search, reboot). See
[External Agents](./guides/mcp.md).

## Restic (Backup) — `restic/*`

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/restic/available` | auth | is `restic` installed |
| GET | `/restic/version` | auth | |
| GET | `/restic/repos` | auth | `?include_password=` (admin only when true) |
| POST | `/restic/repos` | admin | create repo |
| GET/PUT/DELETE | `/restic/repos/{id}` | auth/admin | get/update/delete |
| GET | `/restic/repos/{id}/snapshots` | auth | |
| GET | `/restic/repos/{id}/stats` | auth | |
| GET | `/restic/repos/{id}/jobs` | auth | backup job history |
| POST | `/restic/repos/{id}/unlock` | admin | |
| POST | `/restic/repos/{id}/prune` | admin | |
| POST | `/restic/repos/{id}/mount` | admin | mount the repository read-only for browsing (`restic mount`); the mount point is derived under `{DATA_DIRECTORY}/restic-mounts`, never supplied by the request. Idempotent. `503` when `restic` or FUSE is missing |
| GET | `/restic/mounts` | auth | browse mounts the kernel confirms (`ro,nosuid,nodev`) with `id`, `repo_id`, `repo_name`, `mount_point`, `started_at`, `active` |
| DELETE | `/restic/mounts/{id}` | admin | unmount and stop the FUSE process; `400` for an id that is not a derived directory name |
| POST | `/restic/backup` | admin | **starts** a backup and answers immediately: `{status:"running", job_id, backup_job_id}` (SSE progress, cancellable via `DELETE /progress/{job_id}`) |
| GET | `/restic/reminders` | auth | backup reminders — per repo: `last_backup_at` + `days_since` + `stale`, so the UI can nag when a repository hasn't been backed up recently |
| GET | `/restic/restore-plan` | admin | `?repo_id=&snapshot_id=&target=` — pre-flight space check `{ required_bytes, available_bytes, fits }` (snapshot restore size vs free space on the destination); the restore dialog debounces this before committing |
| POST | `/restic/restore` | admin | **starts** a restore: `{status:"running", job_id}` |
| POST | `/restic/check` | auth | metadata check; `{"read_data": true}` re-reads every pack and is **admin** |
| POST | `/restic/forget` | admin | apply retention — at least one rule required, `keep_within` validated (`30d`, `1y2m`) |
| POST | `/restic/snapshots/delete` | admin | `snapshot_id` validated (`[A-Za-z0-9._-]`, no leading `-`) |
| GET | `/restic/schedules` | auth | |
| POST | `/restic/schedules` | admin | |
| PUT/DELETE | `/restic/schedules/{id}` | admin | |
| PUT | `/restic/schedules/{id}/enabled` | admin | |
| GET | `/restic/retention-policies` | auth | |
| POST | `/restic/retention-policies` | admin | |
| PUT/DELETE | `/restic/retention-policies/{id}` | admin | |

Backup and restore are **background jobs**: the endpoints answer with a
`job_id` before the run starts, progress is streamed from `restic backup --json`,
and the `restic_backup_jobs` row is created (status `running`) before the run —
a job left `running` by a restart is closed at the next startup. One
backup/restore per repository at a time (`RepoRunLock`). Every positional
argument and path is validated in `src/restic.rs` before restic is spawned.

## Docker — `docker/*` (almost entirely `admin`)

Containers, images, networks, compose projects. `handle_docker_available` is
the one deliberate `auth`-only exception (it just gates whether the Docker
app is shown to any logged-in user); `handle_catalog_list` (`GET
/docker/catalog`) is the other. Every other handler shown below as `auth` in
older docs actually calls `require_admin`, including the read/list ones
(`info`, `containers`, `containers/running`, `images`, `hub/search`,
container logs) — Docker exposes real host details (images, container
config, logs), so read access is admin-gated too, not merely session-gated.
Representative endpoints:

| Method | Path | Auth |
|--------|------|------|
| GET | `/docker/info` | admin |
| GET | `/docker/available` | auth |
| GET | `/docker/containers` | admin |
| GET | `/docker/containers/running` | admin |
| GET | `/docker/images` | admin |
| PUT | `/docker/containers/{name}/start\|stop\|restart\|pause\|unpause` | admin |
| PUT | `/docker/containers/{name}/rename` | admin | body `{name}`; `ComposeManager`-style name validation |
| POST | `/docker/containers/{name}/kill` | admin | body `{signal?}` — whitelisted (`ALLOWED_SIGNALS`), unknown ⇒ SIGKILL |
| POST | `/docker/containers/{name}/update` | admin | body `{memory_limit?, cpu_limit?, restart_policy?}` — in place, no recreate |
| POST | `/docker/containers/{name}/recreate` | admin | pull + rebuild from `inspect`, rename-rollback on failure |
| POST | `/docker/containers/{name}/exec` | admin | body `{command: string[], timeout_secs?}` → `{stdout, stderr, exit_code}` |
| GET | `/docker/containers/{name}/inspect` | admin | `ContainerDetail`: state, limits, mounts, ports, networks, env, labels, compose ids |
| GET | `/docker/containers/{name}/logs` | admin | merged stdout+stderr, `?tail=` (1–5000), `?timestamps=` |
| POST | `/docker/containers/{name}/logs/ticket` | admin | single-use 30 s ticket for the SSE stream (EventSource can't send the token header) |
| GET | `/docker/containers/{name}/logs/stream` | ticket | SSE `log` / `end` events; 401 on missing, expired or mismatched ticket |
| POST | `/docker/containers` | admin | create; `no_new_privileges` defaults to true (`--security-opt no-new-privileges:true`), `privileged` is opt-in |
| DELETE | `/docker/containers/{name}` | admin |
| POST | `/docker/images/pull` | admin |
| POST | `/docker/images/build` | admin | body `{context, dockerfile?, tag, no_cache?, pull?, build_args?}`; context through `PathGuard` |
| DELETE | `/docker/images/{image_id}` | admin |
| GET | `/docker/hub/search` | admin |
| GET/POST | `/docker/volumes` | admin | list (driver, mountpoint, `in_use`, `used_by`) / create `{name, driver?}` |
| DELETE | `/docker/volumes/{name}` | admin | `?force=` for a volume still attached to a container |
| GET | `/docker/system/df` | admin | `docker system df` rows + totals |
| POST | `/docker/prune` | admin | body `{target, all?}`; target ∈ `images\|volumes\|containers\|networks\|builders\|system`, anything else rejected **before** the daemon is called |
| GET | `/docker/registries` | admin | signed-in hosts only (`auths` keys of `~/.docker/config.json`) |
| POST | `/docker/registries/login\|logout` | admin | login uses `--password-stdin` |
| GET/PUT | `/docker/daemon/config` | admin | read / validated write of `daemon.json` (+ `systemctl restart docker`) |
| GET/POST/DELETE | `/docker/networks` (+ `/{name}`, `/prune`, `/connect`, `/disconnect`) | admin |
| GET/POST/PUT/DELETE | `/docker/compose/projects` (+ `/{name}/config\|start\|stop\|restart\|update\|services`) | admin | `update` = `compose pull` + `up -d` |
| GET | `/docker/catalog` | auth | manifest-driven app list (16 built-in packages + community stores) + `installed`/`installed_port`; filtered by `resource_shares` `kind=app` for members (admins see all) |
| GET | `/docker/catalog/{id}/icon` · `/gallery/{n}` | auth | package icon / screenshot (`id` is namespaced `<store>:<app>`, URL-encoded) |
| POST | `/docker/catalog/{id}/install` | admin | renders the package's compose template into a project, starts it, writes `.install.toml`, starts the per-app proxy. Body `{port?, alternatives?, environment?, media_share?: {name, share_type}, gpu?}`; hard-gates compatibility, settings, dependencies, the declared `share_mount` and `/dev/dri` |
| GET | `/docker/catalog/{id}/install-review` | auth | pre-install review: dependency plan (satisfiers, providers, selection), declared environment, compatibility verdict, plus `share_mount`, `gpu`, `gpu_available` and the enabled `shares` |
| GET | `/docker/catalog/{id}/logs?tail=N` | auth | tails the installed app's compose logs (1–2000, default 200) |
| DELETE | `/docker/catalog/{id}` | admin | stops + removes the project, stops the proxy |
| POST | `/docker/catalog/{id}/proxy` | admin | toggle the per-app session-cookie auth gate — `{ auth_required, applied }` (`applied: false` = proxy not running, value saved for its next start; an unpersistable marker is an error) |
| GET | `/docker/catalog/updates` · POST `/docker/catalog/{id}/update` | admin | registry-digest update detection (read-only) / `compose pull` + re-render + `up -d` |
| POST | `/docker/catalog/update-all` | admin | sequential update queue over every pending update → `{updated[], failed[]}` |
| GET/PUT | `/docker/updates/schedule` | admin | opt-in scheduled catalog updates: `enabled` (default false), `interval_hours` (6–168), last run/result/error, `docker_available` |
| POST | `/docker/updates/run-now` | admin | one update pass now, same path as update-all → `{report, schedule}` |
| GET/POST/DELETE | `/docker/stores` (+ `/{id}/sync`) | admin | community catalog git repos |
| POST | `/docker/daemon/start` | admin | start the Docker daemon via systemd (**also enables it for boot persistence**) |
| GET | `/docker/daemon/logs` | admin | last 100 lines of the daemon's journal, for diagnosing why it is down |
| GET | `/docker/port-check` | admin | `?port=N` — is a host port free to publish a container on (host `ss` + other containers + HomeVault's web port); re-checked server-side on create/install |

**App catalog (2026-09-03 overhaul):** `src/docker_catalog.rs::CATALOG` is gone.
Apps are packages — a `homevault-app.toml` manifest + a `docker-compose.yml`
template — from the embedded `catalog/` dir plus admin-added community stores.
Wire ids are namespaced `<store>:<app>`; the install key (compose project + data
dir) is the **bare** app id for the built-in `homevault` store, `<store>-<app>`
otherwise. Each installed app gets a dedicated reverse-proxy listener
(47000–47999, `src/app_proxy`) with an optional app-scoped-cookie auth gate;
the app's own published port stays bound to `0.0.0.0` on purpose (the proxy is
an extra entry point, not a lockdown). The proxy's auth gate accepts only the
proxy-scoped `hv_app_session` minted by `GET /app-auth` — never the API
session. See [Docker](./guides/docker.md) and
[Apps Center](./guides/apps-center.md).

Every Compose project-name path parameter is validated by
`ComposeManager::validate_name` before it's joined onto a filesystem path
(path-traversal fix, 2026-07-16 audit).

## Cloud Sync — `rclone/*`

> Previously **every** handler took only `State(_state)` — no session check
> at all. `GET /rclone/config` returns the raw content of the rclone config
> file the server resolved for the request — `rclone_config_path()`: an explicit
> `RCLONE_CONFIG`, else the first existing of rclone's locations, else
> `/etc/rclone.conf`;
> rclone's "obscured" secret encoding is trivially reversible, so this was an
> unauthenticated plaintext credential leak for every configured remote (S3
> keys, OAuth tokens, FTP/SFTP passwords). `sync`/`copy` support a `"local"`
> path type with no restriction, so combined with the missing auth this was
> also an unauthenticated arbitrary-file-read-and-write primitive. Fixed:
> reads use `require_auth`, mutations (including anything that can start a
> transfer, mount a filesystem, or read/write the config) use `require_admin`.

| Method | Path | Auth |
|--------|------|------|
| GET | `/rclone/status` | auth |
| GET | `/rclone/remotes` | auth |
| GET | `/rclone/providers` | auth |
| GET | `/rclone/mounts` | auth |
| POST | `/rclone/mount/start` | admin |
| POST | `/rclone/mount/stop` | admin |
| POST | `/rclone/sync` | admin |
| POST | `/rclone/copy` | admin |
| GET | `/rclone/config` | admin — returns raw config, same sensitivity bar as Restic's `?include_password=true` |
| POST | `/rclone/config` | admin |
| POST | `/rclone/remotes` (add) | admin |
| DELETE | `/rclone/remotes/{name}` | admin |
| GET | `/rclone/jobs` (list) | auth — persisted sync jobs |
| POST | `/rclone/jobs` (create) | admin — optional `schedule_kind` (`none`\|`interval`\|`cron`), `interval_minutes`, `cron_expr`; an unusable cadence is a `400` naming the field |
| DELETE | `/rclone/jobs/{id}` | admin |
| PUT | `/rclone/jobs/{id}/schedule` | admin — `{ schedule_kind, interval_minutes?, cron_expr? }`; `none` clears the due time and keeps the last-run history |
| POST | `/rclone/jobs/{id}/run` | admin — "Run now"; awaits the run and returns the **updated job** (`last_status`, `last_run_at`, `last_message`, `next_run_at`). A scheduled job also runs on its own via the server-side scheduler (ROADMAP 16.4) |

`POST /rclone/crypt` was removed — it was a fake stub (always returned a
canned success string without doing anything) with zero frontend callers.

**Bodies and shapes (2026-10-23 pass).**

- `GET /rclone/status` answers `{"success":true,"data":{"available":bool,
  "fusermount":bool,"mount_base":"/mnt/rclone"}}`. `mount_base` is the
  server's own rule — the screen derives every mount point from it instead of
  hardcoding `/mnt/rclone/<name>`.
- `POST /rclone/mount/start` takes `{"remote","mount_point","path"?,
  "read_only"?,"allow_other"?}`. `daemon` is accepted and ignored: the process
  always owns the mount child (no `--daemon`). `mount_point` must be an
  absolute path **strictly under** `RCLONE_MOUNT_BASE`, with no `./..`
  component; `remote` must be a plain slug and must exist in the config the
  server resolved. Success is reported only once the mount point appears in
  `/proc/self/mounts`; otherwise the error quotes rclone's last stderr lines
  (`"rclone exited before … became a mount point"` /
  `"rclone did not mount … within 20 seconds"`).
- `POST /rclone/mount/stop` takes `{"mount_point"}` **only** (`MountStopRequest`).
  It tries `fusermount -u`, then `umount -f`, then kills the child; a failure
  names both attempts (`"Failed to unmount …: fusermount …; umount …"`).
- Transfer deadlines: a `sync`/`copy`/`jobs/{id}/run` that exceeds six hours is
  killed and answered `{"success":false,"error":"Sync failed: rclone did not
  finish within … seconds"}` (same for `Copy failed:`).
- `POST /rclone/config` writes the config **and** its dated `.backup.*` sibling
  mode `0600`; both are repaired if they were looser.
- `DELETE /rclone/remotes/{name}` removes the exact `[name]` section only —
  `[mydrive]` is untouched when `my` is deleted, and adding `my` beside it is
  allowed.
- Rejections worth matching client-side: `Remote name is required`,
  `Remote name contains invalid characters`, `Provider type contains invalid
  characters`, `Mount point is required`, `Mount point must live under /mnt/rclone`,
  `Remote '<name>' is not configured in the rclone config file`,
  `Something is already mounted at <path>`, `Remote '<name>' not found`.

See [Cloud Sync](./guides/cloud-sync.md) for the deadline table, the mount
lifecycle, the config-hardening rules and the known debt (no scheduler).

## Syncthing — `syncthing/*` (all `admin`)

Each handler calls a local `require_admin` — this closes a previously-real gap
where none of these handlers validated a session at all; see
[Syncthing](./guides/syncthing.md) for what was found and fixed.

| Method | Path | Notes |
|--------|------|-------|
| GET | `/syncthing/status` | instance status |
| GET | `/syncthing/config` | fetched config |
| GET | `/syncthing/devices` | devices |
| GET | `/syncthing/folders` | folders |
| GET | `/syncthing/connections` | live connections |
| GET | `/syncthing/errors` | sync errors |
| POST | `/syncthing/restart` | restart the local instance |
| POST | `/syncthing/pause` | pause one device |
| POST | `/syncthing/resume` | resume one device |
| POST | `/syncthing/scan` | rescan one folder |
| POST | `/syncthing/folders` | add a folder (request below) |
| DELETE | `/syncthing/folders/{id}` | remove a folder; files stay on disk |
| POST | `/syncthing/devices` | pair a device (request below) |
| DELETE | `/syncthing/devices/{id}` | unpair a device; files stay on disk |

### Adding a folder

```http
POST /api/syncthing/folders
{
  "id": "photos",
  "label": "Photos",
  "path": "/mnt/pools/data/photos",
  "type": "sendreceive",
  "devices": ["AAAAAAA-BBBBBBB-CCCCCCC-DDDDDDD-EEEEEEE-FFFFFFF-GGGGGGG-HHHHHHH"]
}
```

`type` is one of `sendreceive`, `sendonly`, `receiveonly` (`receiveencrypted` is
refused); `devices` may be empty or omitted, and every id in it must already be
paired. `label` may be blank (it falls back to the id). Success answers
`{"success":true,"data":{"id":"photos"},"error":null}`.

Rejections, as `{"success":false,"error":…}`:

| Reason | When |
|---|---|
| `Folder ID must not be empty` / `Folder ID is too long` | length |
| `… is not a usable identifier` / `… may only contain letters, digits` | charset outside `[A-Za-z0-9._-]` |
| `Folder name is too long` / `must not contain control characters` | label (128 characters) |
| `Folder path must not be empty` | blank path |
| `Folder path must be an absolute path` | relative |
| `Folder path must not contain '..'` | traversal |
| `Folder path must not be the filesystem root` | `/` |
| `Folder path is too long` | > 1024 bytes |
| `Folder path … must be inside a storage location HomeVault manages` | outside `allowed_roots()` |
| `Folder path must not contain control characters` | newline or NUL |
| `Sync mode '…' is not supported` | mode outside the allowlist |
| `A folder with ID '…' already exists` | duplicate |
| `A folder can be shared with at most 64 devices` | more sharers than the cap |
| `Device ID must not be empty` | blank share target |
| `Device '…' is not paired with this appliance; pair it before sharing a folder with it` | unknown share target |

Validation runs **before** any daemon call, so a stopped daemon cannot turn a
malformed request into a transport error. The path is confined because
Syncthing creates the directory: without the check an admin could create a
folder anywhere the service can write.

### Pairing a device

```http
POST /api/syncthing/devices
{ "device_id": "aaaaaaabbbbbbbcccccccdddddddeeeeeeefffffffggggggghhhhhhh", "name": "Laptop" }
```

The id is normalised: dashes are optional, case is irrelevant, and the canonical
eight-groups-of-seven form is what gets stored. `name` may be blank (it falls
back to the first group of the id). Success answers
`{"success":true,"data":{"device_id":"AAAAAAA-…"},"error":null}`.

Rejections: `Device ID must not be empty`; `Device ID must be 56 characters
(dashes are optional), got N`; `… may only contain letters A-Z and digits 2-7`;
`Device name is too long (128 characters maximum)`; `Device name must not
contain control characters`; `A device with ID '…' is already paired`; and
`This is HomeVault's own device ID; pair the other device instead`.

The check digit is **not** verified here — the daemon is the authority and its
refusal is relayed. `DELETE /syncthing/devices/{id}` refuses the appliance's own
id (`… it cannot be removed`) and re-validates the path segment before it is
interpolated into the daemon's REST path.

Every handler answers HTTP 200 with `{"success":false,"error":…, "data":null}`
on failure (module convention, like Downloader / Cloud Sync). One error is worth
naming because it is a *configuration* fault rather than a transport one: when
the `<apikey>` in the daemon's `config.xml` cannot be sent as a header — a
control character, a non-ASCII byte, or a key the old code would have panicked
on — every endpoint answers
`{"success":false,"error":"Syncthing API key …"}` and `GET /syncthing/status`
reports it instead of `available:false`. A blank `<apikey>` is "no key
configured" (no header sent), and a pretty-printed, indented key is accepted.

## Downloader — `downloader/*` (`aria2`, all `admin`)

Every Downloader endpoint calls `require_admin_request`, so **all** are
admin-only. There are no read-only `auth` endpoints here.

| Method | Path | Notes |
|--------|------|-------|
| GET | `/downloader/status` | `{ available, running, dir }` — `dir` is the download root |
| GET | `/downloader/global-stat` | aria2 global stats |
| GET | `/downloader/tasks` | active/waiting/done tasks |
| GET | `/downloader/task/{gid}` | one task |
| POST | `/downloader/add` | enqueue a download (validation below) |
| POST | `/downloader/start` | start the daemon; returns the canonical download root |
| POST | `/downloader/stop` | stop the daemon |
| POST | `/downloader/pause` | pause a task |
| POST | `/downloader/unpause` | unpause a task |
| POST | `/downloader/remove` | remove a task |
| GET/PUT | `/downloader/settings` | `{ auto_extract }` — unpack finished archives automatically (off by default) |
| POST | `/downloader/extract` | `{ gid }` — unpack the archives a finished download left in its own folder; returns the archive names |
| GET/POST | `/downloader/feeds` | list the RSS/Atom subscriptions / subscribe `{ url, name?, auto_download? }` |
| PUT/DELETE | `/downloader/feeds/{id}` | rename, enable, auto-download / unsubscribe |
| POST | `/downloader/feeds/{id}/refresh` | `{ queue? }` — fetch and list the feed, and queue every new item when `queue` is true |
| POST | `/downloader/feeds/refresh` | check every enabled feed, queueing where the feed opted in; `{ queued, failed }` |

### `POST /downloader/add` — request validation

`{ "uri": string, "dir"?: string, "out"?: string, "max_download_limit"?: string, "max_upload_limit"?: string, "seed_ratio"?: string }`. Every field is validated
**before** the daemon is touched: a refused request returns
`{ success: false, error }` (HTTP 200, like the rest of the module) and never
spawns aria2.

| Field | Rule |
|-------|------|
| `uri` | Required. Scheme allowlist `http`, `https`, `ftp`, `sftp`, `magnet`; plain links need a host, magnet links need `xt=`; no spaces/control characters; ≤ 2048 chars. |
| `dir` | **Relative** sub-folder of the download root (`<DATA_DIRECTORY>/downloads`). No leading `/`, no `..`/`.` segment, no backslash, no control characters; ≤ 128 chars. Absolute paths were accepted before 2026-10-23 and are now refused. |
| `out` | Bare file name only — no path separator, not `.`/`..`, no control characters; ≤ 255 chars. |
| `max_download_limit` / `max_upload_limit` | Digits with an optional `K`/`M`/`G` suffix (`0` = unlimited, ≤ 9 digits), sent as aria2's `max-download-limit` / `max-upload-limit` for that task only. |
| `seed_ratio` | Number 0–100 (`0` = no seeding), sent as `seed-ratio`. |

`dir` is always sent to aria2 explicitly (defaulting to the root), so the daemon
never falls back to its own working directory. The response never contains the
daemon's RPC secret.

### `/downloader/feeds` — RSS subscriptions

A feed URL is validated against the appliance: `http(s)` only, no
`user:password@`, no `localhost`, no literal loopback/RFC 1918 address (the
refresh must not become an SSRF primitive aimed at HomeVault's own services).
The parser (RSS 2.0 and Atom) keeps only items carrying a downloadable URL —
an enclosure, else the item link — and only `http(s)`/magnet; each queued URL
still goes through the `/add` allowlist. A bounded per-feed `seen` marker
(500 ids) records what was actually queued, so a check never consumes items a
later scheduled refresh should pick up. Feeds live in
`data/downloader_feeds.json` (mode 0600).

### `/zfs-cache` — L2ARC / SLOG (admin)

| Method | Path | Notes |
|--------|------|-------|
| GET | `/zfs-cache` | `{ available, pools: [{ pool, state, devices: [{ device, kind: "l2arc"\|"slog", state, read_errors, write_errors, checksum_errors }] }], candidates: [{ name, size, model, rotational }] }`. `available: false` means `zpool` is missing; `candidates` only lists free whole disks |
| POST | `/zfs-cache` | `{ pool, kind, device }` — attaches a cache/log vdev (`zpool add`). `400` when the pool is unknown or not ZFS, the device is not a free whole disk, the pool already has that vdev group, or the disk is spinning (an L2ARC/SLOG needs flash) |
| POST | `/zfs-cache/remove` | `{ pool, device }` — detaches a cache/log vdev (`zpool remove`); refused unless the device is listed as a cache/log device of that pool |

Both mutations are admin-only and journaled. The device path is validated as an
absolute `/dev/…` path (one argv entry, no `..`/whitespace) and the sudoers
grant covers only the `zpool add` / `zpool remove` subcommands.

## Snapshots

> The standalone `/snapshots/{pool}` endpoints and `src/handlers/snapshots.rs`
> were **removed** — every handler there took no `State`/`HeaderMap`, so all
> six endpoints (including destructive delete/rollback) were reachable with
> **no session token whatsoever**. Snapshot functionality now lives where it
> is scoped:
>
> | Area | Endpoints |
> |------|-----------|
> | Explorer (ZFS, read-only browsing) | `GET /explorer/snapshots?path=` (lists covering snapshots with a `browse_path` under `.zfs/snapshot/…`) |
> | Backups (restic) | `GET /restic/repos/{id}/snapshots`, `POST /restic/snapshots/delete` |
> | Virtual machines | `GET/POST /vm/{name}/snapshots`, `POST /vm/{name}/snapshots/{snap}/restore`, `DELETE /vm/{name}/snapshots/{snap}` |
>
> See [Storage](./guides/storage.md) for the Btrfs/ZFS rollback limitation
> (Btrfs rollback of the default mounted subvolume isn't supported and returns
> a clear error, not a silent no-op).

## Progress (SSE) — `progress/*` (`auth`)

`GET /progress/{job_id}` (SSE stream), `GET /progress/jobs`, `GET
/progress/{job_id}/status`, `DELETE /progress/{job_id}` (cancel). Long-running jobs
(backup, restore, clone, etc.) emit `progress` / `done` / `error` / `heartbeat`
events; the SPA subscribes via `EventSource` (`api.subscribeToProgress`).
`/status` answers the job's **real last event** (latest `%`, message, byte
counters and state — `completed`/`failed` included) and a 400 for an id that was
never created or was already removed.

Cancelling runs the job producer's registered hook
(`ProgressManager::register_cancel_hook`) — that is what actually stops the
work; the job then reports `state: "cancelled"`. The response is
`{job_id, cancelled, message}`, and an unknown id is a `400` rather than the
old unconditional "Job cancelled" (which left a `sudo ddrescue` clone running
after the UI said it was stopped). In a `ProgressEvent`, `job_type` is a
tagged object: `{"type":"backup"}` for the built-in kinds,
`{"type":"custom","name":"explorer_copy"}` for a feature job (the enum uses
serde **adjacent** tagging — plain internal tagging cannot serialize the
`Custom` newtype variant, which used to empty the SSE payload for every feature
job, see [guides/explorer.md](./guides/explorer.md)).

## Activity feed — `GET /api/activity` (`auth`)

Unified list for the desktop activity taskbar. Merges the active
`PROGRESS_MANAGER` jobs (backup/restore/verify, cache mover, antivirus scan,
Explorer copy/move, Docker install/update, ddrescue/photorec, RAG reindex,
Ollama model pull, cloud sync) with aria2's active downloads (admin only, and
only when aria2 is already running — the call never starts it) **and** the
synthetic rows a server-side poller writes for live host work HomeVault did not
start: `raid_resync` (`/proc/mdstat`), `pool_scrub` (`zpool status` / `btrfs
scrub status`) and `syncthing_sync` (a folder still catching up) — ROADMAP 28.3.
Non-admins see only jobs they own, so the ownerless synthetic rows are
admin-only. `ApiResponse<ActivityTask[]>` where `ActivityTask =
{ id, kind, state, label, progress: number|null, detail: string|null }`; `state`
is `pending`/`running`/`failed`/`cancelled` — a job that failed or was cancelled
stays in the list for 60 s after its last event so its outcome is visible, and a
`completed` one is never listed (ROADMAP 28.2); `kind` is a
stable slug localised client-side. See [guides/activity.md](./guides/activity.md).

## Rescue — `/rescue/*` (`admin`)

`tools`, `devices`, `ddrescue/mapfile` (`GET`); `ddrescue/clone`,
`ddrescue/status/{log}`, `testdisk/analyze`, `testdisk/rebuild`,
`photorec/recover`, `mdadm/diagnose`, `mdadm/repair` (`POST`). Implemented in
`src/handlers/rescue.rs` + `src/rescue/`. Full semantics in
[guides/rescue.md](./guides/rescue.md).

- `ddrescue/status/{log}` takes a **mapfile path** and answers
  `{running, progress, bytes_copied, bytes_total, speed, message}`. The path is
  confined to `/tmp`, `/var/tmp` and `paths::rescue_dir()` and must be an
  existing regular file that is not a symlink (≤ 16 MiB) — a missing file is
  not an error, it means no mapfile has been written yet. `ddrescue/mapfile`
  reports the directory the per-job default mapfiles live in.
- Only options the packaged tools can honour are accepted; anything else is a
  `400` naming the field (see the option table in `guides/rescue.md`). That
  includes TestDisk's `deep_search`/`log_path`/`force` and PhotoRec's
  `keep_broken`/`max_size`, which used to be accepted and ignored.
- A clone is refused when the destination is the same device, is in use
  (mounted / active swap / md / LVM / dm-crypt member) or is smaller than the
  source.
- Clones and carves can be stopped: `DELETE /progress/{job_id}` now runs the
  job's cancellation hook, which SIGTERMs the tool's process group.

## VPN — `/vpn/*` (all `admin`)

Every VPN endpoint requires an **admin** session — the whole surface is
privileged (it writes `/etc`, brings up `wg0` and launches root daemons), so
there are no read-only `auth` endpoints here. Router in `src/handlers/vpn.rs`,
engine in `src/vpn.rs`; full semantics in [guides/vpn.md](./guides/vpn.md).

| Method | Path | Notes |
|--------|------|-------|
| GET | `/capabilities` | `{capabilities:{wireguard,openvpn}}` — which backends are installed |
| GET | `/connections` | legacy flat list — `{connections: [...]}` |
| GET | `/connections/{id}` | `{connection}` |
| GET | `/wireguard/server` | server status; **provisions `wg0` on first call** (`10.8.0.1/24`, port 51820) |
| GET/PUT | `/wireguard/endpoint` | admin | public endpoint used in peer configs: resolved (manual → DDNS → detected public IP) or set/cleared (`{host}`) — returns `endpoint_host`, `endpoint_source`, `endpoint_warning` |
| GET | `/wireguard/peers` | provisioned devices + live handshake status (`connected` = handshake < 180 s) |
| POST | `/wireguard/peers` | add device — `{name}` only; response carries the one `client_config` |
| GET | `/wireguard/peers/{id}/config` | re-display the device's client config/QR (keys stored per migration 026) |
| DELETE | `/wireguard/peers/{id}` | revoke device (live + persisted) |
| GET | `/wireguard/clients` | imported outbound client tunnels + live state (`available`, `clients[]`); never the config/private key |
| POST | `/wireguard/clients` | import `{name,config_content,kill_switch?,autostart?}` — allowlist parser; **400** invalid/hook, **409** duplicate name, **503** without `wireguard-tools` |
| POST | `/wireguard/clients/{id}/start` | write config, `wg-quick up`, verify the interface within 15 s, roll back on failure |
| POST | `/wireguard/clients/{id}/stop` | `wg-quick down` (idempotent) |
| PUT | `/wireguard/clients/{id}` | `{autostart}` — reconnect at boot |
| DELETE | `/wireguard/clients/{id}` | disconnect, remove `/etc/wireguard/hvc-*.conf`, forget the record |
| POST | `/openvpn` | upload profile — `{name,config_content}`; **content-validated** (no exec directives / `script-security > 1`, else 400 naming the directive), **409** if the same name's profile is currently running |
| POST | `/openvpn/{name}/start` | start + **verify the tunnel connects within a 10 s grace** (log marker), else the daemon is killed and the API answers `success:false` with the log's error line; body `{username,password}` optional (`--auth-user-pass`) |
| POST | `/openvpn/{name}/stop` | kill daemon, delete its `.log` and `.auth` files |
| DELETE | `/openvpn/{name}` | stop, then delete profile |

Payloads use the app's standard `{success, message?, data}` envelope. The
OpenVPN start request intentionally blocks up to ~10 s (verify-then-rollbac
k); the frontend shows a per-card connecting spinner meanwhile.

## Virtual Machines — `/vm/*` (all `admin`)

Incus-backed VMs and system containers. Every endpoint requires an **admin**
session (Incus uses a root socket). Router for the console WebSocket in
`src/handlers/vm.rs::create_vm_router`; the REST routes are registered directly
in `src/web/mod.rs`. Full detail in [Virtual Machines](./guides/vm.md).

| Method | Path | Notes |
|--------|------|-------|
| GET | `/vm/available` | `{ available }` — Incus daemon reachable |
| GET | `/vm/os-options` | curated OS catalogue (`OsOption[]`); no Incus needed |
| GET | `/vm/list` | `VmInfo[]` — status, IPv4, snapshot count (one `recursion=2` call) |
| POST | `/vm/create` | `CreateVmRequest` → `{ name, status }`; image pull runs as a `vm_create` progress job |
| POST | `/vm/{name}/clone` | `{ name, snapshot?, profiles? }` → `{ name, source, snapshot, status }`; Incus copies config/devices, the clone stays stopped; `status:"cloning"` + operation id if the 300 s wait expires; `503` when Incus is missing |
| GET | `/vm/{name}` | `VmDetail` |
| DELETE | `/vm/{name}` | stops first if running |
| PUT | `/vm/{name}/start` \| `/stop` \| `/restart` | waits for the operation |
| PUT | `/vm/{name}/config` | `{ cpu_cores?, memory_mb? }` |
| GET/POST | `/vm/{name}/snapshots` | list / create `{ name, stateful? }` |
| POST | `/vm/{name}/snapshots/{snap}/restore` | stop → restore → restart if it was running |
| DELETE | `/vm/{name}/snapshots/{snap}` | |
| POST | `/vm/console/ticket` | `{ name, cols?, rows? }` → `{ ticket }` (single-use, 20 s) |
| GET | `/vm/console/ws` | `?ticket=…` WebSocket → interactive shell (xterm) |
| POST | `/vm/{name}/screen/ticket` | `{ ticket }` — ISO-install VM's graphical screen (must be running + have a screen) |
| GET | `/vm/screen/ws` | `?ticket=…` WebSocket → VM VNC (noVNC), subprotocol `binary` |
| GET | `/vm/isos` | managed ISO store (`{DATA_DIRECTORY}/vm/isos`) — `IsoEntry[]` |
| POST | `/vm/isos/download` | `{ url, name? }` — fetch an https `.iso` (progress job) |
| POST | `/vm/isos/virtio` | one-click latest `virtio-win.iso` |
| DELETE | `/vm/isos/{name}` | remove a managed ISO |

`CreateVmRequest`: `name` (1–63 `[a-z0-9-]`, lower-cased), `size`
(`small`|`medium`|`large`), `cpu_cores?`/`memory_mb?`/`disk_gb?`, plus **one of**:
ready-made — `kind` (`vm`|`container`), `os` (catalogue id), `username`
(default `admin`), `password` (6–128, seeded via cloud-init as a SHA-512 hash);
or ISO — `iso_path` (absolute `.iso`, exists, ≥ 10 MB), `iso_os`
(`linux`|`windows`), `drivers_iso_path?`, `secure_boot?`.

## Wallpapers

**No API.** The upload feature was removed 2026-09-04. The 44 bundled wallpapers
are static assets served at `GET /wallpapers/{*path}` (registered before the SPA
fallback). See [Wallpapers](./guides/wallpapers.md).

## Avatars

`GET /avatars/preview` (public, raw SVG), `POST /auth/me/avatar` (auth, set seed).
User avatars served at `GET /users/{username}/avatar.svg`.

## Terminal — `terminal/*` (all `admin`)

Router in `src/handlers/terminal_pty.rs`, nested at `/api/terminal`. The PTY
WebSocket requires a single-use ticket (long-lived tokens never appear in URLs).
Every endpoint is admin-only; `GET /settings/public` exposes
`terminal_enabled` so the desktop can hide the entry point when an admin
disabled the terminal (`403 Terminal is disabled` on ticket issuance). Ticket
issuance and tab close log the username, tab id and client IP.

| Method | Endpoint | Notes |
|--------|----------|-------|
| POST | `/terminal/ticket?tab=` | issue a single-use WS ticket |
| POST | `/terminal/close?tab=` | tear down one shell tab |
| GET | `/terminal/cwd?tab=` | real shell cwd (via `/proc/<pid>/cwd`) — feeds completion + prompt |
| GET | `/terminal/ls?tab=&path=` | directory listing for the completion popup |
| WS | `/terminal/ws?ticket=` | the shell (ticket redeemed once) |

## Disk quotas — no endpoints

The per-group filesystem-quota feature (`quotas/*` routes, `handlers/quotas.rs`)
was **removed 2026-09-09**. The only disk quota left is the **per-user
home-directory size limit** (`setquota -u` / `zfs set userquota@`, verified by
a `repquota`/`zfs get` read-back), which has no dedicated endpoint — it rides
on the Users home-folder calls (`POST /users`, `PUT /users/{u}/full`,
`PUT /users/{u}/home`) and its live state is exposed by
`GET /users/{u}/home`. Distinct from **transfer quotas** (per-user/group
upload/download caps, `transfer-quotas/*`). See
[Disk Quotas](./guides/quotas.md).

## SMS alerts — inside `settings/notifications` (admin)

The SMS channel has no dedicated route: it rides the notifications settings as
an `sms` object (phase B3). See [SMS](./guides/sms.md).

| Field | Meaning |
|---|---|
| `enabled` | dispatch on the same events/threshold as email and webhook |
| `endpoint` | URL to POST to |
| `body_template` | body with `%to%` and `%text%` placeholders |
| `recipient` | E.164 (`+33612345678`) — `400` when it is not |
| `extra_headers` | optional `[[name, value], …]` |
| token | sealed under the `sms-token` scope before it is stored |

**`GET` answers a redacted view**: `{ enabled, endpoint, body_template,
recipient, extra_headers, token_set }` — there is no field that could carry the
credential. On `PUT`, a **blank** token means "keep the stored one", which is
the only way to change the number without re-typing a secret the API never
returns. A configuration that is disabled is never validated, so a half-filled
form can always be cleared.

## Notifications — `notifications/*` (`auth`)

Broadcast-style system notifications (health alerts, backup results), populated
by the background engine in `src/notification_engine.rs`. Read state is
**per user** (ROADMAP 21.2): the feed is shared, but each account's unread
view lives in the `notification_reads` join table, so marking read only ever
affects the caller. Deleting the shared feed is admin-only. See
[guides/notifications.md](./guides/notifications.md).

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/notifications?limit=&offset=` | auth | `handle_notifications_list` — paginated (limit clamped to ≤200, default 50), newest first, caller's `read_at` resolved; answers `{ notifications, total, limit, offset }` |
| GET | `/notifications/unread-count` | auth | `handle_notifications_unread_count` — the **caller's** badge count |
| POST | `/notifications/read-all` | auth | `handle_notifications_mark_all_read` — caller only |
| POST | `/notifications/{id}/read` | auth | `handle_notification_mark_read` — caller only; `404` on an unknown id |
| DELETE | `/notifications/{id}` | admin | `handle_notification_delete` — `404` on an unknown id |
| DELETE | `/notifications` | admin | `handle_notifications_clear_all` — bulk clear (ROADMAP 21.3), answers `{ deleted }` |
| GET | `/notifications/events/config` | admin | `handle_notification_events_config_get` — event catalog + per-event state + `webhook_format` |
| PUT | `/notifications/events/config` | admin | `handle_notification_events_config_update` — `{ events?: {id: bool}, webhook_format?: "generic"\|"slack"\|"discord" }` |

## Web Push — `notifications/push/*` (`auth`)

Per-device browser push (phase D2). Consent is personal, so these routes are
not admin-only: each account manages its own devices, and the three
subscription secrets (endpoint, `p256dh`, `auth`) are never returned. The VAPID
private key is sealed with the appliance key; only the public key is exposed.
See [guides/web-push.md](./guides/web-push.md).

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/notifications/push` | auth | `handle_push_get` — `{ available, public_key, subscriptions[], max_devices, error }`; `available: false` (with `error`) when the VAPID keys cannot be prepared |
| POST | `/notifications/push/subscribe` | auth | `handle_push_subscribe` — `{ endpoint, keys: { p256dh, auth } }`; the endpoint must be HTTPS on 443 and resolve to a public address, `p256dh` a 65-byte uncompressed P-256 point, `auth` exactly 16 bytes; at most 10 devices per account |
| DELETE | `/notifications/push/subscribe/{id}` | auth | `handle_push_unsubscribe` — revokes one of the **caller's** devices; `404` for a foreign or unknown id |

A subscription is stored only after the browser permission and this POST; a
`404`/`410` from the push service removes the device, and a VAPID key
regeneration leaves every existing device requiring re-enable.

## What's New — `whats-new/*` (`auth`)

Per-user release highlights shown once per version. See
[guides/notifications.md](./guides/notifications.md) → "What's New".

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/whats-new` | auth | `handle_whats_new` — `{ version, title, highlights, seen }` |
| POST | `/whats-new/seen` | auth | `handle_whats_new_seen` — per-user acknowledgement (idempotent) |

## Client/server contract — `/api/meta` (public)

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/meta` | public | `handle_meta` — `{ contract_version, server_version }` |

The SPA sends `X-HomeVault-Contract: <n>` on every API request; a present but
different value is rejected with **426 Upgrade Required** by
`middleware::contract_middleware` (`/api/meta` exempt). A missing header is
allowed for legacy clients. See [ARCHITECTURE.md](./ARCHITECTURE.md) →
"Versioned client/server contract".

## OpenAPI 3 — `/openapi.json` (admin)

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/openapi.json` | admin | `handle_openapi_json` — an OpenAPI **3.0.3** document |

`src/openapi.rs` builds the document from the **router source itself**
(`src/web/mod.rs` plus the `/api/network`, `/api/vpn`, `/api/rescue`,
`/api/terminal` and `/api/vm` sub-routers). There is no second route table to
maintain: a route added to `create_app` appears here on the next build, and a
removed one disappears. The endpoint is `require_admin` — the document names
every administrator-only operation, so it is not a public map of the
appliance.

The generated document declares:

- the `sessionToken` (`X-Session-Token`) and `csrfToken` (`X-CSRF-Token`)
  security schemes, the optional `X-HomeVault-Contract` parameter, and the
  `x-homevault-contract-version` extension;
- the standard `400` / `401` / `403` / `426` / `500` / `501` / `503` responses
  (public operations omit `401`/`403`);
- `security: []` on the public rows of this file, so a client can tell an
  authentication flow from a session-only operation;
- `x-homevault-auth: public|session` on every operation. Administrator-only
  operations still advertise `session` because the administrator check is a
  role decision inside the same session and answers `403`; see
  `handlers/admin_guard.rs`.

**Excluded on purpose:** WebSocket, server-sent-event, one-shot-ticket and
binary upload/download transports are not JSON request/response endpoints.
They are omitted from `paths` and listed in `x-homevault-excluded` with a
reason, so an omission is never mistaken for a missing route. The exclusions
live in `EXCLUDED_TRANSPORTS` (`src/openapi.rs`); a new transport must be added
there (the unit tests fail if a ticket or terminal route leaks into `paths`).

```bash
curl -s -H "X-Session-Token: $TOKEN" https://nas.local/api/openapi.json | jq .
```

The document is cached once per process and serialized with sorted keys, so
its bytes are stable across two reads (asserted by
`tests/openapi_integration.rs`). Unlike the hand-curated sections of this
file, the generated spec carries no request/response schemas: those remain
documented per area below and in `guides/`.

## Antivirus (ClamAV) — `antivirus/*`

Read endpoints `auth`; scan/settings mutations `admin`. See
[Antivirus](./guides/antivirus.md).

| Method | Path | Auth |
|--------|------|------|
| GET | `/antivirus/status` | auth |
| GET/PUT | `/antivirus/settings` | auth / admin |
| POST | `/antivirus/scan` | admin |
| POST | `/antivirus/scan/cancel` | admin |
| GET | `/antivirus/scans` | auth |
| GET | `/antivirus/threats` | auth |
| POST | `/antivirus/threats/{id}/resolve` | admin |

## Firewall (ufw) — `firewall/*`

Read endpoints `auth`; every mutation is `admin`. Enabling **and** disabling
require `{"confirmed": true}` in the body. Deleting a rule selects a live rule
(by its `spec`, or by port/protocol/action/source) — an absent rule is a 404
and a mandatory SSH/app-port rule is a 403, both checked against live `ufw`
state. See [Firewall](./guides/firewall.md).

| Method | Path | Auth |
|--------|------|------|
| GET | `/firewall/status` | auth |
| GET | `/firewall/enable/preview` | auth |
| GET | `/firewall/history?limit=` | auth |
| POST | `/firewall/enable` | admin |
| POST | `/firewall/disable` | admin |
| POST | `/firewall/default-policy` | admin |
| POST/DELETE | `/firewall/rules` | admin |
| GET | `/firewall/geoip` | auth | `handle_geoip_status` — `available`, `ufw_available`, `database_present`, `database_path`, `database_error`, `countries[]` (`country`, `action`, `cidr_count`), `managed_rules`, `max_rules` |
| POST | `/firewall/geoip` | admin | `handle_geoip_rule_set` — `{ country (2-letter ISO), action (allow|deny) }`; expands the country's CIDRs and reconciles the managed `hv-geoip:*` ufw rules. Missing ufw/database → `503`; bad country/action/network → `400` |
| DELETE | `/firewall/geoip/{country}` | admin | `handle_geoip_rule_delete` — removes the country policy and its managed rules |
| POST | `/firewall/geoip/apply` | admin | `handle_geoip_apply` — reconciles the current policy after a manual database refresh; `503` when ufw or the database is missing |

## Anti-Ransomware — `ransomware/*`

Read endpoints `auth`; settings/protected-path mutations `admin`. See
[Anti-Ransomware](./guides/ransomware.md).

| Method | Path | Auth |
|--------|------|------|
| GET | `/ransomware/status` | auth |
| GET/PUT | `/ransomware/settings` | auth / admin |
| GET/POST | `/ransomware/protected-paths` | auth / admin |
| DELETE | `/ransomware/protected-paths/{id}` | admin |
| GET | `/ransomware/events` | auth |

## Cache tier — `cache/*`

Read endpoints `auth`; settings/share/mover mutations `admin`. See
[Cache](./guides/cache.md).

| Method | Path | Auth |
|--------|------|------|
| GET/PUT | `/cache/settings` | auth / admin |
| GET | `/cache/pools` | auth |
| GET/POST | `/cache/shares` | auth / admin |
| PUT | `/cache/shares/{name}` | admin |
| POST | `/cache/mover/run` | admin |
| GET | `/cache/mover/runs` | auth |

`PUT /cache/settings` validates `mover_cron` (5 fields), `min_free_percent`
(1–95), `mover_age_days` (≤3650), `mover_bwlimit` (≤10,000,000 KB/s) and
`cache_pool` (must exist). Sending `cache_pool: null` **or** `cache_pool: ""`
clears the designation; omitting the field leaves it untouched. Share create
and update both validate `target_pool` (exists, not the cache pool).

`GET /cache/mover/runs` returns the last 50 runs, each with
`shares: [{ share_name, mode, status, files_moved, bytes_moved, message }]`
(`status` = `moved | skipped | failed`). Run `status` is
`completed | partial | failed | running` — `partial` means some shares moved
and some did not; `error_message` then names the ones that did not.

## System Journal — `journal/*`

The `app` source is session-level; **every host and audit source is `admin`** —
that is the whole `/var/log/auth.log` surface and the queryable audit trail
(account names, source addresses, actions), so it is gated per source, not per
screen. `GET /journal/sources` only advertises what the caller may read.
Clear-logs and cleanup-settings writes are `admin`. See
[System Journal](./guides/journal.md).

| Method | Path | Auth |
|--------|------|------|
| GET | `/journal/sources` | auth (lists `app`; adds `audit`, `system` + the `/var/log` files for an admin) |
| GET | `/journal/entries` | auth for `?source=app`, admin for `audit`/`system`/`syslog`/`auth`/`kern`/`daemon` (403 otherwise) |
| GET | `/journal/export` | same gate as `/journal/entries` | the current filtered view as `format=csv\|json` (≤ 5 000 entries), returned as a download |
| DELETE | `/journal/clear` | admin |
| GET/PUT | `/journal/cleanup-settings` | auth / admin |
| GET | `/audit` | admin — paged audit trail (`from`, `to`, `severity`, `actor`, `action`, `q`, `limit` ≤ 500, `offset`); returns `entries`, `total`, `retention_days` |
| GET | `/audit/export` | admin — same filters + `format=csv\|json` (≤ 5 000 rows), returned as a download |

`GET /journal/entries` answers
`{ "success": true, "data": [ … ], "meta": { "scanned": 200, "limit": 200, "search_truncated": false } }`.
`search_truncated` is `true` when a search filter filled the bounded scan
window (`limit × 20`, at most 10 000 lines), i.e. older matching entries exist
beyond what was examined — the screen says so instead of showing a short list.
An unknown `source` is a `400`, and a `unit` containing anything but letters,
digits, `.`, `-`, `_`, `@` and `:` is a `400` naming the accepted characters.

## Transfer quotas — `transfer-quotas/*`

Per-user/per-group upload/download caps, enforced against File Explorer
transfers only (not SMB/NFS/FTP/WebDAV/SFTP shares — those are served by
separate system daemons this codebase can't meter). See
[Transfer Quotas](./guides/transfer-quotas.md).

| Method | Path | Auth |
|--------|------|------|
| GET | `/transfer-quotas/{subject_type}` | admin |
| GET | `/transfer-quotas/{subject_type}/{subject_name}` | self or admin |
| PUT | `/transfer-quotas/{subject_type}/{subject_name}` | admin |
| DELETE | `/transfer-quotas/{subject_type}/{subject_name}` | admin |
| GET | `/transfer-quotas/history` | admin | hourly usage samples for one subject (`subject_type`, `subject_name`, `limit` ≤ 500), newest first, 90-day retention; feeds the Quotas history dialog and the 90% crossing alerts |

`subject_type` ∈ {`user`, `group`}. A user's own quota and every group they
belong to are each checked and debited independently.

"Self or admin" means the single-subject `GET` answers only when
`subject_type = user` **and** the caller is that user, or when the caller is
an admin — another household user gets a `403`, and no group quota is ever
readable by a member. The list `GET` is admin-only because its payload carries
every subject's limits *and* consumed bytes.

A row serialises `period` (`lifetime`\|`day`\|`week`\|`month`),
`period_started_at` and the computed `period_resets_at` (absent for a
lifetime quota). `PUT` accepts the same `period`; an unknown value is a named
error listing the accepted ones, and the field defaults to `lifetime` when
absent. Changing the period starts a fresh window (counters zeroed); editing
only a limit keeps the counters. See
[Transfer Quotas](./guides/transfer-quotas.md) for the calendar-window rules
and the fail-closed behaviour of the File Explorer guard.

## UPS / SNMP — `ups/*`, `snmp/*` (all `admin`)

Status/inventory endpoints for a locally-attached UPS (via NUT) and
SNMP-discoverable network devices. `snmp/settings` has a `PUT`; the UPS
`config/*` endpoints manage the device sections of `/etc/nut/ups.conf`
(serial / network / USB — a USB UPS is normally auto-detected by the
`usbhid-ups` `port = auto` section the installer writes). Both have real
frontend cards in Settings (`UpsCard.svelte`/`SnmpCard.svelte`) — see
[Settings](./guides/settings.md).

| Method | Path | Auth |
|--------|------|------|
| GET | `/ups/availability` | admin |
| GET | `/ups/list` | admin |
| GET | `/ups/status/{name}` | admin |
| GET | `/ups/variables/{name}` | admin |
| GET | `/ups/config` | admin — list `ups.conf` sections |
| POST | `/ups/config` | admin — add/update a section (`{name, driver, port, desc?}`; name and driver validated, reloads NUT). `data.reload_warnings` names every unit that refused to restart — `success: true` means *written*, not necessarily *live* |
| DELETE | `/ups/config/{name}` | admin — remove a section (reloads NUT; same `reload_warnings` contract) |
| GET | `/snmp/availability` | admin |
| GET | `/snmp/info` | admin — uses the saved community/version (a `community` query parameter overrides the community) |
| GET | `/snmp/interfaces` | admin — `?iface=<name>` narrows the table (matched against `ifName`, case-insensitive) |
| GET | `/snmp/storage` | admin |
| GET/PUT | `/snmp/settings` | admin — `version` is `1` or `2c`; `3` answers `400` because the card does not collect v3 credentials |
| GET | `/snmp/agent` | admin — the appliance's **own** agent: `{ enabled, scope, lan_address, version, v3_username, location, contact, has_community, has_v3_auth_secret, has_v3_priv_secret, installed, serving, lan_candidates[] }`. Neither the community nor a v3 passphrase is ever returned. **`503`** when `snmpd` is not installed (capability, not a bad request) |
| PUT | `/snmp/agent` | admin — `{ enabled?, scope?, lan_address?, version?, community?, v3_username?, v3_auth_secret?, v3_priv_secret?, location?, contact? }` → the same payload. `scope` is `loopback` (127.0.0.1 only) or `lan` (one of the appliance's private addresses — `0.0.0.0` and public addresses are `400`). `version` is `v2c` (default) or `v3` (`authPriv`: SHA authentication, AES privacy). `public`/`private` communities are `400`; an empty `community` or v3 passphrase keeps the stored one or generates a 32-hex one. v3 passphrases are sealed at rest (`hv1:`) and never echoed back. Validated before anything is written, applied to the host, persisted only once the host accepted it; a failed daemon step re-applies the previous configuration and names the failure. See [`guides/snmp-agent.md`](./guides/snmp-agent.md) |

## DHCP / TFTP / PXE server — `network/dhcp-server/*`

Serves IPv4 addresses through `dnsmasq` on one interface, and optionally
boot files over TFTP/PXE. Configuration lives in
`{DATA_DIRECTORY}/data/dhcp_server.json` (a single document, no migration).

**Guard order matters.** Every mutation takes the raw request body and
deserializes it *after* `require_admin` succeeds, so an unauthenticated caller
always gets `401` with the standard JSON envelope and learns nothing about the
payload shape. Extracting the typed body first (the shape before this fix)
answered `422` and echoed the deserializer's message.

| Method | Path | Auth |
|--------|------|------|
| GET | `/network/dhcp-server` | auth — `{ config, state, available }`; `state` carries `running`, `leases[]`, `conflict_detected`, `last_error` |
| PUT | `/network/dhcp-server` | admin — `{ config }`; validates, checks for a conflict, saves **and applies** |
| POST | `/network/dhcp-server/start` | admin — starts from the saved config; `400` when both switches are off |
| POST | `/network/dhcp-server/stop` | admin — **`409`** while the local DNS server (which shares the daemon) is on |
| GET | `/network/dhcp-server/leases` | auth — the live lease table |

A second address server already answering on the interface is a **`409`**, never
a silent takeover. Validation and the conflict probe both run before anything is
written, so a refused configuration leaves the previous one and the daemon
untouched.

## Local DNS server — `network/dns-server`

The appliance's own resolver (phase C1). It is the **same** `dnsmasq` instance
as the DHCP server — one daemon, one drop-in
(`/etc/dnsmasq.d/homevault.conf`, composed by `dhcp_server::render_conf`), two
configuration documents. Configuration lives in
`{DATA_DIRECTORY}/data/local_dns.json`.

| Method | Path | Auth |
|--------|------|------|
| GET | `/network/dns-server` | auth — `{ config, state, available, interfaces[], system_forwarders[], dhcp_enabled }`; `state` carries `running` (the shared daemon) and `serving` (running **and** this feature on) |
| PUT | `/network/dns-server` | admin — `{ config }`; validated, persisted, then applied by reconciling the shared daemon; a host refusal restores and re-applies the previous document and names the failure |

`config` is `{ enabled, interface, serve_dhcp_clients, forwarders[], records[], overrides[], confirmed }`:

- `records[]` — `{ name, kind: "a" | "cname", value }`. A `cname` target must be
  an `a` record from the same list (dnsmasq would silently ignore it
  otherwise), names are lowercased and RFC 1035 validated;
- `overrides[]` — `{ domain, address }` rendered as `address=/<domain>/<address>`;
  a leading dot keeps the match on subdomains only;
- `forwarders[]` — at most 3 upstream resolvers, never loopback;
- `serve_dhcp_clients` — drop the DHCP card's `dhcp-option=6` list so dnsmasq
  advertises itself; `400` while the DNS server is off.

Enabling requires an interface, at least one forwarder and `confirmed: true`.
Every refusal is a `400` whose message names the rule. See
[`guides/local-dns.md`](./guides/local-dns.md).

## Generic reverse proxy — `proxy/rules`

User-defined `host [+ path] → upstream` rules (phase C2), served by **one**
host-routed listener (default port `80`, `HOMEVAULT_REVERSE_PROXY_PORT`
overrides it). Rules live in the `reverse_proxy_rules` table; the listener
keeps an in-memory snapshot, reloaded after every mutation. A bind failure is
reported by the status and never stops the dashboard.

| Method | Path | Auth |
|--------|------|------|
| GET | `/proxy/rules` | auth — `{ rules[], listener: { listening, port, error }, max_rules }` |
| POST | `/proxy/rules` | admin — `{ host, path, upstream, tls, auth_required, enabled }`; validated, DNS-resolved, then persisted |
| PUT | `/proxy/rules/{id}` | admin — same body; `404` for an unknown id, `400` for a duplicate `(host, path)` |
| DELETE | `/proxy/rules/{id}` | admin — removes the rule and revokes its sessions and tickets |
| GET | `/proxy-auth?rule=<id>&next=<path>` | **browser cookie** — validates `hv_session`, mints a one-time ticket (120 s, single use, rule-bound) and redirects to the listener |

Validation refusals are `400` and name the rule they broke: a host with a port
or scheme, a wildcard, a path prefix without `/`, an upstream that is not
`host:port`, a port of `0`, an upstream that resolves to a **public** address,
or an upstream that points at the listener's own port or the dashboard port.
`tls` means the **upstream** speaks HTTPS; the listener itself is plain HTTP.
`auth_required` (default `true`) gates the rule with an `hv_app_session`
scoped to `proxy:<id>`; the browser obtains it through the cross-host ticket
exchange, never by sending `hv_session` upstream. See
[`guides/reverse-proxy.md`](./guides/reverse-proxy.md) and the design note
[`analysis/c2-reverse-proxy-design.md`](./analysis/c2-reverse-proxy-design.md).

## Router port forwarding — `network/upnp`

UPnP-IGD mappings through the `upnpc` client from **miniupnpc** (phase C3).
Opening the screen is read-only; every mutation is admin-only and must carry
`confirmed: true`. The appliance stores no router credential.

| Method | Path | Auth |
|--------|------|------|
| GET | `/network/upnp` | auth — `{ available, gateway, external_ip, local_address, mappings[], error, suggested_host }`; `available: false` when `upnpc` is absent (no error), `error` carries the router's own refusal otherwise |
| POST | `/network/upnp/mappings` | admin — `{ protocol: "tcp"\|"udp", external_port, internal_host, internal_port, description, lease_seconds, confirmed }`; adds one mapping, then returns the refreshed state |
| DELETE | `/network/upnp/mappings` | admin — `{ protocol, external_port, confirmed }`; removes one mapping, then returns the refreshed state |

`internal_host` must be a **private, link-local or unique-local IP literal**;
loopback, public addresses and host names are `400`. Ports are `1..=65535` and
the lease is `0` (permanent) or `60..=604800` seconds; the default is `3600`.
The description is printable ASCII, at most 64 characters including the
`HomeVault` prefix, with no quotes or backslashes. A missing `confirmed` is a
`400` before anything is sent to the router; a missing `upnpc` on a mutation is
a `503` naming `miniupnpc`; a router refusal is a `502` with its own sentence.
See [`guides/upnp.md`](./guides/upnp.md).

## USB one-touch copy — `usb-copy/*`

Copy rules that pull files off a removable device into a folder on the
appliance. Profiles live in `{DATA_DIRECTORY}/data/usb_copy_profiles.json`.

| Method | Path | Auth |
|--------|------|------|
| GET | `/usb-copy/profiles` | auth |
| POST | `/usb-copy/profiles` | admin — body is the profile itself |
| PUT | `/usb-copy/profiles/{id}` | admin — the path id wins over the body id |
| DELETE | `/usb-copy/profiles/{id}` | admin |
| GET | `/usb-copy/devices` | auth — attached removable devices, each with `matching_profile` |
| POST | `/usb-copy/execute/{profile_id}` | admin — schedules the copy (`202`-style answer: `{ status, message }`) |

`POST …/execute` answers as soon as the task is spawned; the task writes
`last_run` / `last_status` itself, so an outcome is visible even when nothing is
polling. A disabled profile is a `400`; a source that is a pool member is a
`400`; an unknown profile or an absent device is a `404`.

## Fan and thermal control — `system/fans/*`

hwmon channels, temperature curves and the active profile. The saved profile
lives in `{DATA_DIRECTORY}/data/fan_control.json`.

| Method | Path | Auth |
|--------|------|------|
| GET | `/system/fans` | auth — `{ channels[], is_laptop, message }` |
| GET | `/system/fans/config` | auth |
| PUT | `/system/fans/config` | admin — **applies** before saving, so a refused profile never becomes the persisted one |
| POST | `/system/fans/restore-auto` | admin — hands control back to the firmware **and** resets the saved profile to `auto` |

A laptop is refused with a `400` and a sentence. The 20 % duty floor is enforced
in `set_pwm_duty` and in `calculate_duty`, so no curve, manual value or
per-channel override can go below it.

## UPS server mode — `power/ups-server/*`

Exposes the appliance as a NUT server. Config lives in
`{DATA_DIRECTORY}/data/ups_server.json`.

| Method | Path | Auth |
|--------|------|------|
| GET | `/power/ups-server` | auth — `{ available, config, state }` |
| PUT | `/power/ups-server` | admin — validates, saves **and applies** (restarts `upsd` or stops it) |
| POST | `/power/ups-server/start` | admin — `400` when the feature is not enabled |
| POST | `/power/ups-server/stop` | admin |
| POST | `/power/ups-server/shutdown` | admin **+ `confirm: true`** |

`POST …/shutdown` answers:

```json
{ "success": false, "stages": [
  { "stage": "stop_containers", "success": true,  "skipped": false, "duration_ms": 812, "error": null },
  { "stage": "shutdown_vms",    "success": false, "skipped": true,  "duration_ms": 0,
    "error": "VM shutdown is not wired to the Incus service yet" }
] }
```

- **`skipped` is a first-class field.** A stage the policy enabled but this
  build cannot perform is `success: false, skipped: true` with a reason — never
  a silent `success: true`. Two stages are in that state today
  (`shutdown_vms`, `flush_restic`); both used to return `Ok(())` for work they
  did not do.
- **The top-level `success` is `false` when any enabled stage was skipped**, so
  "did not happen" is never reported as "done".
- **The shutdown runs in a fixed order**: `stop_containers` → `shutdown_vms` →
  `flush_restic` → `stop_shares` → `final_poweroff`. `final_poweroff` always
  runs whatever the policy says.
- **A body without `confirm: true` is a `400` naming the rule**, never a silent
  no-op.
- Reserved ports are `0`, `22`, `80`, `443` and `8443` — the last because it is
  HomeVault's own alternative HTTPS listener.

## Guided redundancy — `storage/guided-redundancy`

| Method | Path | Auth |
|--------|------|------|
| GET | `/storage/guided-redundancy?min_redundancy=none\|single\|double` | auth — `{ recommendations[], total_disks, available_disks }` |
| POST | `/storage/guided-redundancy/apply/{id}` | admin → **`501`** |

The recommendation engine is complete. **Creating the array is not
implemented**: `apply` answers `501` with a sentence pointing at the Storage
screen, and the dialog says so before the click. The route is admin-only from
the start so the privilege boundary does not have to be renegotiated when the
creation path lands. The disk inventory handed to the engine is currently empty
— connecting it to `disk.rs` is the precondition for the feature being useful
end to end.

## Desktop monitor state — `monitor-layout`, `weather-location` (`auth`)

Per-user persisted UI state, not tied to any specific monitor widget:
`GET/POST /monitor-layout` (drag position + visibility for every desktop
monitor panel) and `GET/POST /weather-location` (the Weather monitor's saved
city). See [Widgets](./guides/widgets.md).

`POST /monitor-layout` takes `{layout_json: "<client blob>"}` and stores that
string **verbatim** (upsert on `username`) so a read returns exactly the bytes
that were written. Before storing it is validated structurally — the blob is
never re-serialised:

| Bound | Value |
|-------|-------|
| Type | a JSON **object** (an array/scalar is `400`) |
| Size | ≤ 16 KiB |
| Nesting | ≤ 4 levels |
| Nodes (objects + arrays + scalars) | ≤ 512 |
| Object key length | ≤ 64 characters |
| String value length | ≤ 256 characters |

A rejected blob is `400` and leaves the stored layout untouched.

## Web shortcuts — `shortcuts/*` (`auth`)

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| POST | `/shortcuts/fetch-metadata` | auth | `handle_fetch_metadata` — `{url}` → `{url, title, description, icon_url, site_name}` |

The server fetches the page (SSRF-guarded: http/https only on **every hop**,
each resolved address must be public, redirects followed manually up to 5 hops
with the target re-checked before it is dialled, connect-time address pinning
through a validating resolver, 512 KB read while streaming, and one 10 s budget
for the whole chain). Failures are `400` with a `message` the dialog shows
verbatim (`Only http and https URLs are allowed`, `Refusing to fetch a private
or local address`, `Too many redirects (at most 5 are followed)`, `The site
took too long to respond`, …). See [Web shortcuts](./guides/shortcuts.md).

## Events (SSE) — `/api/events`, `notifications/events/ticket` (`auth`)

Same single-use-ticket pattern as the Terminal WebSocket (a long-lived token
must not travel in a URL query string): `GET /notifications/events/ticket`
(auth) issues a short-lived ticket, then the actual `GET /events?ticket=...`
SSE stream (no header-based auth — the ticket is one-time use) pushes live
notification events to the client.

## Scheduled Tasks — `scheduled-tasks/*` (admin)

The generic cron engine (commands, pool snapshots, SMART self-tests, host
power actions). See [guides/scheduled-tasks.md](./guides/scheduled-tasks.md).

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/scheduled-tasks` | admin | `handle_tasks_list` |
| POST | `/scheduled-tasks` | admin | `handle_task_create` — payload validated at creation; optional `timezone` (IANA name). `task_type: "service"` triggers an existing subsystem (`docker_update`, `trash_purge`, `db_backup`, `notification` with a required `message`) |
| GET | `/scheduled-tasks/{id}` | admin | `handle_task_get` — one task, the status endpoint of a started run |
| PUT | `/scheduled-tasks/{id}` | admin | `handle_task_update` — resets the run summary |
| POST | `/scheduled-tasks/{id}/enabled` | admin | `handle_task_set_enabled` — `{"enabled": bool}`, toggles only (keeps the run summary) |
| DELETE | `/scheduled-tasks/{id}` | admin | `handle_task_delete` |
| POST | `/scheduled-tasks/{id}/run` | admin | `handle_task_run_now` — runs regardless of cron; **`202 Accepted`** with `{run_id, task_id, status:"running"}` (the execution continues outside the request — ROADMAP 12.4); `409` when a run of the same task is already in progress (process-wide single-flight guard shared with the scheduler tick) |
| GET | `/scheduled-tasks/{id}/runs` | admin | `handle_task_runs` — 50 newest |

## Metrics history — `metrics/*` (auth)

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/metrics/history?range=1h\|24h\|7d` | auth | `handle_metrics_history` — bucketed averages (`bucket_seconds` + `points[]`); every series is nullable and a gap is `null`, never `0` |
| GET | `/metrics/latest` | auth | `handle_metrics_latest` — newest stored sample; `200` with `data: null` when nothing has been collected yet (fresh install, not a client error) |

Read-only: samples are written only by the `metrics` collector loop. A series
is `null` when its `/proc` (or thermal-zone) read failed — see
[guides/metrics.md](./guides/metrics.md#null-semantics--a-gap-is-not-a-zero).

## Host power — `power/*` (admin)

See [guides/power.md](./guides/power.md). `/power/{reboot,shutdown}` are the
host-side aliases of `/server/{restart,shutdown}` — one primitive, one
privilege path, two JSON shapes (see *Server control* above).

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/power/status` | admin | `handle_power_status` — governor + capability flags |
| PUT | `/power/governor` | admin | `handle_power_governor_set` — `{ governor }` |
| POST | `/power/reboot` | admin | `handle_power_action` — host reboot (`sudo systemctl reboot`) |
| POST | `/power/shutdown` | admin | `handle_power_action` — host poweroff (`sudo systemctl poweroff`) |
| POST | `/power/suspend` | admin | `handle_power_action` — host suspend (`sudo systemctl suspend`) |
| GET | `/power/spindown` | admin | `handle_power_spindown_get` |
| PUT | `/power/spindown` | admin | `handle_power_spindown_set` — `{ device, apm_level?, spindown_minutes? }`, applied immediately |
| POST | `/power/spindown/apply` | admin | `handle_power_spindown_apply` — re-apply all saved settings |
| GET | `/power/wol/hosts` | admin | saved Wake-on-LAN machines + the interface evidence a packet would use (`interfaces[]`: `name`, `address`, `prefix`, `broadcast`, `up`), `ports` and `max_hosts` |
| POST | `/power/wol/hosts` | admin | `{ label, mac, broadcast? }`; the MAC is normalised to `AA:BB:CC:DD:EE:FF`, `broadcast` must be an IP literal, the label is unique case-insensitively (`400` names the problem) |
| PUT | `/power/wol/hosts/{id}` | admin | edit a machine (`400` on a duplicate name or a vanished row) |
| DELETE | `/power/wol/hosts/{id}` | admin | forget a machine |
| POST | `/power/wol/wake` | admin | `{ id }` or `{ mac }` → `{ sent: true, report: { mac, destination, interface, ports }, message }`; the message says **sent, not confirmed**, because a magic packet is an unacknowledged datagram and the target is off |
| (task payload) | — | — | any scheduled-task payload may carry `"wake": { "host_id", "delay_seconds" }`; validated on save (delay ≤ 600 s) and again before the run |

## Snapshot schedules — `snapshot-schedules/*`

Cron-driven pool snapshots with GFS retention. See
[guides/snapshots.md](./guides/snapshots.md).

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/snapshot-schedules` | admin | `handle_snapshot_schedules_list` |
| POST | `/snapshot-schedules` | admin | `handle_snapshot_schedule_create` — `{ pool, share?, share_type?, cron, prefix, recursive, gfs_hourly..yearly, enabled }`; `share` + `share_type` are supplied together (per-share target, P1-12) or both omitted (pool-wide) |
| PUT | `/snapshot-schedules/{id}` | admin | `handle_snapshot_schedule_update` |
| DELETE | `/snapshot-schedules/{id}` | admin | `handle_snapshot_schedule_delete` — snapshots are kept |
| POST | `/snapshot-schedules/{id}/run` | admin | `handle_snapshot_schedule_run_now` — create + GFS prune now |
| GET | `/snapshot-schedules/snapshots?pool=` | auth | `handle_snapshot_pool_list` — pool snapshots, `auto` tags the schedules' own prefix |

- `GET …/snapshots` is a **read** (`require_auth`): it only lists names.
  Everything that mutates or destroys stays `require_admin`.
- **409 Conflict** — `POST`/`PUT` when another schedule already owns the
  same `(pool, prefix)` pair (unique index, migration 053), and `POST
  …/{id}/run` when a run already holds that schedule or its pool
  (single-flight; nothing was attempted, the row is untouched).
- **400** — `POST`/`PUT` for a pool the host does not report (skipped when
  the host reports no pool at all), plus the usual cron/prefix/pool-name
  validation.
- A run whose prune fails is recorded as `last_status: "failed"` with the
  reason in `last_error`, and raises one `snapshot.schedules` notification
  per outage.

## Snapshots — immutable locks — `snapshots/locks*`

A lock is a row **and** a filesystem hold (`zfs hold homevault-lock` /
`btrfs property set ro true`), so a snapshot cannot be destroyed before its
horizon even by a direct command. See
[guides/snapshots.md](./guides/snapshots.md#immutable-snapshots-2026-10-31-roadmap-p25).

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/snapshots/locks?pool=` | auth | `handle_snapshot_locks_list` — active locks (`break_glass_phrase` per row), the 50 most recent released rows, `max_lock_days`, `tag` |
| POST | `/snapshots/locks` | **admin** | `handle_snapshot_lock_create` — `{ pool, snapshot, days, reason? }`; the hold is applied before the row, and rolled back if the row cannot be written |
| DELETE | `/snapshots/locks/{id}` | **admin** | `handle_snapshot_lock_release` — body `{ confirmation? }`; before the horizon it must equal `release <snapshot-name>` |

- **Refused with 200/`success:false`** (the house style): an already-locked
  snapshot (`"… is already locked until …"`), a horizon outside 1–3650 days, a
  reason over 200 characters or with control characters, and an early release
  whose `confirmation` does not match the phrase.
- A release after the horizon needs no phrase. An early one is journaled as
  `BREAK-GLASS: …` and keeps `release_reason` on the row.
- A lapsed lock is **not** treated as released: `snapshot_lock_scheduler`
  releases the hold first and only then marks the row, so an unreachable pool
  keeps the snapshot protected.

## Backup destination — `backup-target/*`

Receive `restic` backups from other machines, one append-only repository per
tenant. See [guides/backup-target.md](./guides/backup-target.md).

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/backup-target` | auth | `handle_backup_target_state` — settings, tenants, `repos_root`, `rest_server_available`, `restic_available`, `service_active` |
| PUT | `/backup-target/config` | **admin** | `handle_backup_target_settings_update` — `{ enabled, port, listen }` |
| POST | `/backup-target/tenants` | **admin** | `handle_backup_tenant_create` — `{ name, quota_bytes }`; returns `token`, `repository_password`, `repository_url` **once** |
| PUT/DELETE | `/backup-target/tenants/{id}` | **admin** | quota and verification toggle / delete the tenant row (repository left on disk) |
| POST | `/backup-target/tenants/{id}/rotate` | **admin** | new token + repository password, returned once |
| POST | `/backup-target/tenants/{id}/verify` | **admin** | `restic check` now (10-minute ceiling) |

- **Secrets never come back.** Reads expose `has_token` and the last check
  outcome; the token is stored as SHA-256 and the repository password sealed
  with the appliance key. The `htpasswd` file `rest-server` reads carries a
  `{SHA}` entry — acceptable only because the password is machine-generated.
- **Quota enforcement is revocation**: over quota, the tenant is removed from
  the htpasswd file and every request is refused until the quota is raised.
- The generated unit is `homevault-rest-server` with `--append-only
  --private-repos`; it is started only when the role is enabled.

## Snapshot replication — `replication/*`

Scheduled `zfs send`/`btrfs send` to another dataset (local or over SSH). See
[guides/replication.md](./guides/replication.md).

| Method | Path | Auth | Handler |
|--------|------|------|---------|
| GET | `/replication` | auth | `handle_replication_jobs_list` — jobs + `capabilities` (`zfs`, `btrfs`, `ssh`, `pv`, limits) |
| POST | `/replication` | **admin** | `handle_replication_job_create` |
| PUT/DELETE | `/replication/{id}` | **admin** | update / delete (existing copies stay on the destination) |
| GET | `/replication/{id}/preview` | auth | `handle_replication_preview` — the exact next-run pipeline, a dry run |
| POST | `/replication/{id}/run` | **admin** | `handle_replication_run_now` |

- Validation refuses: a name over 64 characters or with control characters, a
  dataset containing `@` (the job creates its own snapshots), Btrfs over SSH,
  a local destination equal to the source, an interval other than 0 or
  5–43200 minutes, `keep_last` outside 1–365, and a bandwidth cap above
  10 000 000 kB/s.
- A run is single-flight per job (`run_job` holds a process-wide registry): a
  second `run` answers `success:false` instead of interleaving two sends of the
  same dataset. A missing `zfs`/`ssh` binary is refused up front for the backend
  and destination the job uses.
- `bandwidth_note` reports a cap that could not be applied (no `pv`).

## Health endpoint detail

`GET /health` (`handle_health_endpoint`, `src/web/mod.rs`) requires a valid
session, awaits `HealthService::run_all_checks(&pool)` (9 checks —
authentication, storage, memory, disk_space, disk_smart, **raid_arrays**,
network, services, temperatures; `raid_arrays` reports degraded/failed mdadm
arrays as a warning/error and raises `health.raid_arrays` notifications), and
reads CPU/memory from `/proc/stat` and `/proc/meminfo`. The checks run on
`spawn_blocking` inside `HealthService` (no subprocess on the async runtime)
and degrade independently — a failing check becomes an `unknown` slot, so a
single broken dependency can never blank the payload. Returns
`{ success, data: { checks[], overall_status, cpu_usage, memory_usage } }` —
note the field names are `checks`/`overall_status`, not `metrics`/`overall`.
See [Health](./guides/health.md).

## Related docs

- [Backend Structure](./BACKEND.md) — where each handler lives.
- [Frontend Structure](./FRONTEND.md) — `api.ts` client methods.
- Per-area guides under [guides/](./guides/).
