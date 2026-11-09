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
| GET | `/vpn/wireguard/clients` | admin | imported outbound client tunnels + live state; **never** the configuration/private key |
| POST | `/vpn/wireguard/clients` | admin | import — `{ name, config_content, kill_switch?, autostart? }`; validated allowlist, **400** invalid, **409** duplicate name, **503** without `wireguard-tools` |
| POST | `/vpn/wireguard/clients/{id}/start` | admin | write the config, `wg-quick up`, verify the interface, roll back on failure |
| POST | `/vpn/wireguard/clients/{id}/stop` | admin | `wg-quick down` (idempotent when already down) |
| PUT | `/vpn/wireguard/clients/{id}` | admin | `{ autostart }` — connect at boot |
| DELETE | `/vpn/wireguard/clients/{id}` | admin | disconnect, remove the config file, forget the record |
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

## WireGuard client tunnels (P1-17)

The **client** direction is independent from the server/peer feature above:
the appliance itself can join a provider or a remote concentrator. An admin
imports a standard `.conf`; HomeVault validates it, writes its own copy under
`/etc/wireguard/hvc-<id>.conf` (0600), brings the interface up and can
reconnect it at boot.

**The configuration is parsed from an allowlist, never passed through.**

- `[Interface]`: `PrivateKey`, `Address`, `DNS`, `MTU`, `ListenPort`.
- `[Peer]` (one or more): `PublicKey`, `PresharedKey`, `AllowedIPs`,
  `Endpoint`, `PersistentKeepalive`.
- Refused with a named message: `PreUp`/`PostUp`/`PreDown`/`PostDown`
  (`wg-quick` runs them as root — the same privilege-escalation class as the
  OpenVPN deny list), `SaveConfig` (HomeVault owns the file), `Table`/`FwMark`
  (routes come from `AllowedIPs`) and every unknown directive.
- Every value is validated before it is **re-rendered** into the file: keys
  must decode to 32 bytes, addresses/DNS/`AllowedIPs` must parse, `Endpoint`
  must be `host:port` (IPv6 in brackets), the port is 1–65535. A value can
  therefore never inject a second line.

`kill_switch = true` appends two **fixed** `wg-quick` hooks (no user text):
`PostUp`/`PreDown` install and remove `iptables` OUTPUT rules for traffic that
does not leave through `%i`, with `ip6tables` skipped when absent. Without the
option the rendered file carries no hook at all.

**Interface naming and privileges.** The display name is never used as an
interface or path: `wireguard_client_interface(id)` derives `hvc-` + 10
characters from the record uuid (14 bytes, Linux's limit is 15), so a client
can never reach `wg0`. The sudoers grants are pinned to that namespace
(`wg show hvc-*[/dump]`, `wg-quick up|down hvc-*`); the config directory is
already `homevault`-owned, so the file itself is written directly.

**Persistence and boot.** Records live in
`{DATA_DIRECTORY}/data/wireguard_clients.json`; the rendered configuration is
sealed with the appliance key (`hv1:`, scope `wireguard-client:<id>`), so the
private key never appears in an API payload or the list endpoint. A data
directory restored on another machine yields an unreadable blob: the API says
so and the admin re-imports the `.conf` instead of running half-configured.
At startup, after `WG_CLIENT_BOOT_DELAY` (20 s), every record with
`autostart = true` is reconnected by `restore_autostart_clients`; a failure is
logged and the record is kept.

**Start is verify-then-rollback.** The file is rewritten from the sealed blob
before each start, `wg-quick up` runs under `VPN_WRITE_TIMEOUT`, then the
kernel is polled for 15 s (`wg show <iface> dump`) — `wg-quick` can return
success while `PostUp`/resolvconf failed. On timeout the interface is brought
down. An import that fails at any point removes the interface, the file and
the record, in that order
(`wireguard_client_import_rollback_plan`).

**Limits.** Only the standard `wg-quick` field set is imported; provider
configs that rely on `Table = off`, `SaveConfig` or custom scripts are
refused with the directive named (they must be adapted first). The kill
switch is IPv4 `iptables` with an optional IPv6 companion — it is not a
replacement for a firewall policy on the LAN side.

## WireGuard verification

`ensure_wireguard_server` re-checks that `wg0` is actually up after each
`wg-quick up` (300 ms grace) instead of assuming success — `wg-quick` can
fail silently (missing permissions, PostUp script error) and provisioning
would otherwise announce a server no peer can reach. Sudoers grants live in
`src/setup.rs` (`HostBins::vpn_grants`, scoped since ROADMAP 7.4): `wg show
wg0`, `wg show wg0 dump`, `wg set wg0 …`, `wg-quick up wg0`, plus the client
namespace `wg show hvc-*[/dump]` and `wg-quick up|down hvc-*` (nothing else
of either binary — the interface is either the hardcoded `WG_INTERFACE` or a
HomeVault-generated `hvc-*` name),
`openvpn --config /etc/openvpn/*.ovpn --cd /etc/openvpn --log
/etc/openvpn/*.log --daemon [--auth-user-pass /etc/openvpn/*.auth]` (both
tails, so the optional credentials file stays inside the config directory),
`kill -TERM <pid>` (the signal is pinned; the pid cannot be expressed in
sudoers — a documented residual, and it is validated numeric upstream),
`systemctl enable/start wg-quick@wg0`, and scoped `tee` on
`/etc/wireguard/wg0.conf`; `/etc/wireguard`
and `/etc/openvpn` are chowned `homevault:homevault 0750` by
`prepare_vpn_directories`, which is what lets the unprivileged service write
the configs directly.

## Deadlines on every host tool (2026-10-24, ROADMAP X.2)

Every command of this module used to be a bare `Command::output()` (or, for the
two stdin cases, a hand-rolled `.spawn()` + `wait_with_output()`). A `wg`
blocked on a missing kernel module, an `openvpn --daemon` parent that never
exits or a `pgrep` on a huge process table pinned the calling thread with no
ceiling — and the VPN screen polls the server status and the peer list. Each
call now goes through `crate::utils::process::run_timed` behind one local
`run_command(cmd, timeout)` helper:

| Call | Deadline | Why |
|---|---|---|
| `wg show`/`--version`/`genkey`/`genpsk`, `pgrep`, `ip route`, `sudo cat`/`test` | `VPN_PROBE_TIMEOUT` 10 s | one-shot read; the screen polls it |
| `wg set`, `wg-quick up`, `sudo chmod`, `systemctl enable`, `sudo kill` | `VPN_WRITE_TIMEOUT` 30 s | root op; can wait on the sudo/PAM stack or the kernel |
| `openvpn --config … --daemon` | `VPN_WRITE_TIMEOUT` 30 s | `run_timed_detached` — see below |

Two stdin cases (`wg pubkey`, `wg set … preshared-key /dev/stdin`) go through
`run_timed_with_stdin`, which feeds the input from its own thread and closes
stdin afterwards. The OpenVPN daemon start uses the new
`utils::process::run_timed_detached`: the parent forks and exits at once but the
daemon keeps the inherited write ends of any pipe, so `run_timed`'s drain
threads would never see EOF and its deadline could never fire — the helper
detaches both streams to `/dev/null` and only waits for the exit status.
`run_command` keeps a non-zero exit as *data* (`wg show` on a stopped interface
means "not running"). A source guard
(`test_no_subprocess_of_this_module_runs_without_a_deadline`) fails if a bare
`.output()`/`.spawn()`/`.status()`/`wait_with_output()` comes back, and requires
`run_timed_detached(` to stay in the daemon path.

## Public endpoint detection (P1, 2026-11-09)

Peer configs embed the endpoint a phone or laptop dials. It is resolved in
this order:

1. a **manual override** saved by an admin (`data/wg_endpoint.json`) — a DNS
   name or an IPv4/IPv6 address, validated before it is stored;
2. the **Dynamic DNS hostname** when DDNS is enabled in Settings;
3. the **detected public IPv4** (`discover_public_ip` in `src/network.rs`:
   `HOMEVAULT_PUBLIC_IP` env override, else three echo endpoints
   `api.ipify.org` / `ifconfig.me` / `icanhazip.com`; a hit is cached **15
   min**, a miss **60 s**).

`GET /api/vpn/wireguard/endpoint` returns `endpoint_host`, `endpoint_source`
(`manual`/`ddns`/`public_ip`/`unset`) and `endpoint_warning`; `PUT` sets or
clears the override. The VPN card shows the value and its source, and warns
when the value is unset or a private/LAN/CGNAT address — the case where a
config that looks valid cannot work from outside. The generated config keeps
the `YOUR-SERVER-IP` placeholder as a last resort, but the warning is on
screen before a peer is created.

## Invariants

1. every mutation is admin-only and event-logged (`log_event`/`log_event_warn`);
2. the peer's private key is stored for config re-display (migration 026)
   but never in list payloads, never streamed to the logs;
3. OpenVPN credentials never sit on disk longer than the connection (deleted
   on stop/delete), `.auth` files are 0600;
4. no uploaded profile can execute a command — the engine re-validates
   whatever a stale upload might contain;
5. a failed start never leaves a half-connected daemon behind;
6. every host command carries a deadline (see above); `run_command`,
   `run_timed_with_stdin` and — for the daemonizing OpenVPN parent —
   `run_timed_detached` are the only allowed shapes. A bare `Command::output()`
   in this module is a regression the source guard test catches;
7. an imported client configuration is parsed from an allowlist, re-rendered,
   sealed at rest and never returned by the API; `PreUp`/`PostUp`/`PreDown`/
   `PostDown` can never reach `wg-quick`.

## Tests

- `src/vpn.rs` unit (47): peer-IP allocation, config extraction, client
  config builder, saved-config listing, **profile-content deny list matrix**
  (per-directive, comments/values must not match, case-insensitivity,
  `script-security` levels), **log parsers** (`Initialization Sequence
  Completed` marker, fatal/transport error extraction, timestamp stripping
  in the summary), **subprocess deadlines** (source guard, kill path,
  exit-status-as-data, missing tool, deadline ordering), and **12 on the
  imported WireGuard client** (allowlist fields, multi-peer route merge,
  `PreUp`/`PostUp`/`PreDown`/`PostDown` refusal, unknown directive,
  key/CIDR/DNS/endpoint validation, missing sections, render/round-trip,
  fixed kill-switch hooks, bounded `hvc-` naming, stored-interface
  validation, exact argv, rollback order, confined config path);
- `tests/vpn_integration.rs` (34): guards on every endpoint, name
  sanitisation (traversal, encoded slash, control characters, empty,
  oversized), read-only success paths, **400 profile-content tests** (exec
  hook, `script-security 2`, credential collector), and **9 client tests**
  (401/403, empty name, invalid config, `PostUp` refusal, empty list, 404
  matrix, id traversal) — they fail validation before any file is written so
  they stay host-independent;
- `tests/e2e/vpn/vpn.spec.ts` (44, Playwright) — §15 covers the client tab,
  import modal fields, button gating, `.conf` upload, and the two 400 paths;
- the genuine WireGuard/OpenVPN tunnel behaviour cannot be exercised in the
  Rust suites (root interfaces, real daemons) — see `TESTING.md`, live pass.

Full detail: [API.md](../API.md) § VPN.
