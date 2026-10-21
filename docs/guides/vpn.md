# VPN (WireGuard / OpenVPN)

**Purpose:** Manage the appliance's VPN endpoints. **Every** VPN endpoint
requires an **admin** session — there are no read-only `auth` endpoints (the
whole surface is privileged: it writes `/etc`, brings up `wg0` as root and
launches root daemons).

## Design

Two independent backends behind one router (`src/handlers/vpn.rs`
`create_vpn_router`, nested at `/api/vpn`; engine in `src/vpn.rs`):

- **WireGuard — one always-on server + N roadwarrior devices.** `wg0`
  (address `10.8.0.1/24`, listen port 51820/UDP) is provisioned on first use
  by `ensure_wireguard_server` (keys, config at `/etc/wireguard/wg0.conf`,
  `wg-quick up wg0`, `systemctl enable wg-quick@wg0`) and stays on. Devices
  (phones, laptops) are provisioned as peers through the API: the only input
  is a display name, keys and the next free `10.8.0.x` address are generated
  server-side, the peer is attached live (`wg set wg0 peer …`) and persisted
  in SQLite (migration `025`, plus `026` which stores private/preshared keys
  so a client config can be re-displayed later — the list endpoint never
  returns them). Revocation removes the peer live **and** from the persisted
  config.
- **OpenVPN — client profiles.** The admin uploads a `.ovpn` provider
  profile ( commerciale VPN provider or a remote office concentrator );
  HomeVault saves it under `/etc/openvpn/<name>.ovpn` and can start/stop the
  daemon (`openvpn --config … --daemon` as root via the sudoers grant).

## Endpoints

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/vpn/capabilities` | admin | which backends are installed — payload is `data.capabilities` |
| GET | `/vpn/connections` | admin | legacy flat list (WireGuard `wg0` + saved OpenVPN profiles) — `data.connections` |
| GET | `/vpn/connections/{id}` | admin | one connection — `data.connection` |
| GET | `/vpn/wireguard/server` | admin | server status card (public key, port, detected public endpoint, running); **provisions the server on first call** |
| GET | `/vpn/wireguard/peers` | admin | provisioned devices + live handshake status (`connected` = handshake < 180 s) |
| POST | `/vpn/wireguard/peers` | admin | add device — `{ name }`; response carries the one `client_config` |
| GET | `/vpn/wireguard/peers/{id}/config` | admin | re-display the device's client config/QR (keys were stored at creation) |
| DELETE | `/vpn/wireguard/peers/{id}` | admin | revoke: removed from `wg0` live, then from the config and DB |
| POST | `/vpn/openvpn` | admin | upload profile — `{ name, config_content }`; content-validated, refuses to overwrite a running one (409) |
| POST | `/vpn/openvpn/{name}/start` | admin | start + **verify connect** (body may carry `{ username, password }` for `--auth-user-pass`) |
| POST | `/vpn/openvpn/{name}/stop` | admin | kill daemon + delete its log and credential files |
| DELETE | `/vpn/openvpn/{name}` | admin | stop, then delete profile + defence files |

Payload convention: the same `{success, message?, data}` envelope every
other area uses (`VpnCapabilitiesResponse` etc. once returned bare payloads
the frontend's `response.success` check misread).

## OpenVPN start is a verified operation

`openvpn --daemon` forks and exits 0 immediately — the old code answered
"started" even when the TLS handshake never completed (wrong credentials,
unreachable endpoint, bad profile) and the daemon silently retried forever.
`start_openvpn_connection` is now a **start → verify → rollback** unit:

1. refuses when a daemon already runs the same profile (idempotent success);
2. logs to a per-connection file (`/etc/openvpn/<name>.log`, removed fresh at
   every start), and runs with `--cd /etc/openvpn` — `--daemon` **chdir()s to
   /**, which breaks every relative path inside the profile (verified
   empirically against OpenVPN 2.6);
3. credentials, when supplied, go to `/etc/openvpn/openvpn-<name>.auth`
   (mode 600). **The file is kept while the connection runs** — OpenVPN
   re-reads it on renegotiation — and is deleted on stop/delete;
   `/run` is deliberately not used (not writable by the service user).
   Note the client-side `--status` file does **not** carry connection state
   (only statistics counters) — that is why the verify polls the log;
4. the log is polled every 250 ms for at most **10 s** for the
   `Initialization Sequence Completed` marker; a fatal
   (`Options error`, `Exiting due to fatal error`) short-circuits the wait;
5. on timeout or fatal, the daemon is **killed (rollback)** and the API
   answers `success:false` with the log's error line (the timestamp is
   stripped by `openvpn_log_error_summary`). A start failure is also
   written to the event log (`log_event_warn`).

The handlers wrap the engine calls in `tokio::task::spawn_blocking` — the
start request may take the full 10 s grace, and the frontend shows a
per-card connecting spinner while it flies (key `vpn.connecting`).

## OpenVPN profile validation (deny list)

The daemon runs as root, so an uploaded profile that can execute external
commands is a privilege escalation. `validate_openvpn_config_content`
refuses, at save time **and again at handler level** (400 naming the
directive):

- the exec hooks: `up`, `down`, `down-pre`, `route-up`, `ipchange`,
  `learn-address`, `client-connect`, `client-disconnect`,
  `auth-user-pass-verify`, `tls-verify`, `askpass`;
- `script-security` at level > 1 (level 1, the default, runs nothing and
  stays legal).

Matching is whole-directive (case-insensitive), so `remote
backup-up.example.com` or a comment never trips the list. The handler also
answers **409** on an upload whose name matches a profile a daemon is
currently running from (stop the connection before replacing it).

## WireGuard verification

`ensure_wireguard_server` re-checks that `wg0` is actually up after each
`wg-quick up` (300 ms grace) instead of assuming success — `wg-quick` can
fail silently (missing permissions, PostUp script error) and provisioning
would otherwise announce a server no peer can reach. Sudoers grants live in
`src/setup.rs` (`wg`, `wg-quick`, `openvpn`, `kill`, `systemctl enable/start
wg-quick@wg0`, scoped `tee` on `/etc/wireguard/wg0.conf`); `/etc/wireguard`
and `/etc/openvpn` are chowned `homevault:homevault 0750` by
`prepare_vpn_directories`, which is what lets the unprivileged service write
the configs directly.

## Public endpoint detection

The server card and the generated client configs embed the detected public
IPv4 (`discover_public_ip` in `src/network.rs`: `HOMEVAULT_PUBLIC_IP` env
override, else three echo endpoints `api.ipify.org` / `ifconfig.me` /
`icanhazip.com`). A hit is cached **15 min**, a miss **60 s**
(`PublicIpCache`), so status polls and peer provisioning no longer mean an
external HTTPS request per page view. When nothing resolves, the client
config ships the `YOUR-SERVER-IP` placeholder the admin must replace.

## Invariants

1. every mutation is admin-only and event-logged (`log_event`/`log_event_warn`);
2. the peer's private key is stored for config re-display (migration 026)
   but never in list payloads, never streamed to the logs;
3. OpenVPN credentials never sit on disk longer than the connection (deleted
   on stop/delete), `.auth` files are 0600;
4. no uploaded profile can execute a command — the engine re-validates
   whatever a stale upload might contain;
5. a failed start never leaves a half-connected daemon behind.

## Tests

- `src/vpn.rs` unit (29): peer-IP allocation, config extraction, client
  config builder, saved-config listing, **profile-content deny list matrix**
  (per-directive, comments/values must not match, case-insensitivity,
  `script-security` levels), **log parsers** (`Initialization Sequence
  Completed` marker, fatal/transport error extraction, timestamp stripping
  in the summary);
- `tests/vpn_integration.rs` (25): guards on every endpoint, name
  sanitisation (traversal, encoded slash, control characters, empty,
  oversized), read-only success paths, **400 profile-content tests** (exec
  hook, `script-security 2`, credential collector) — they fail validation
  before any file is written so they stay host-independent;
- `tests/e2e/vpn/vpn.spec.ts` (39, Playwright);
- the genuine WireGuard/OpenVPN tunnel behaviour cannot be exercised in the
  Rust suites (root interfaces, real daemons) — see `TESTING.md`, live pass.

Full detail: [API.md](../API.md) § VPN.
