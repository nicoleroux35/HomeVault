# DHCP / TFTP / PXE server

**Purpose:** let the appliance hand out IPv4 addresses on a network the admin
controls, and optionally serve boot files so other machines start from it.
Useful on an isolated lab or home LAN where the router's own DHCP is turned
off.

**Who can use it:** any signed-in account may read the configuration, the live
state and the lease table. Saving, starting and stopping are **admin-only** —
they drive `dnsmasq` through `sudo systemctl`.

## Where it lives

- **Screen:** Settings → Network → **DHCP server** →
  `frontend/src/lib/components/settings/DhcpServerCard.svelte`.
- **Backend:** `src/dhcp_server.rs` (config validation, `dnsmasq` config
  generation, daemon lifecycle, lease parsing) and
  `src/handlers/dhcp_server.rs` (HTTP surface).
- **Storage:** `{DATA_DIRECTORY}/data/dhcp_server.json` — a single document, so
  no migration.
- **Dependency:** `dnsmasq` (`src/dependencies.rs`, offered by
  `installer/install.sh` as an optional package). When it is absent the card
  still loads and names the missing package.

## Endpoints

| Method | Path | Guard |
|--------|------|-------|
| `GET`  | `/api/network/dhcp-server` | `require_auth` |
| `PUT`  | `/api/network/dhcp-server` | `require_admin` — save **and apply** |
| `POST` | `/api/network/dhcp-server/start` | `require_admin` |
| `POST` | `/api/network/dhcp-server/stop` | `require_admin` |
| `GET`  | `/api/network/dhcp-server/leases` | `require_auth` |

`GET` answers `{ config, state, available }`. `state` carries `running`, the
`leases[]` table, `conflict_detected` and `last_error`.

## Rules

- **The guard runs before the body is parsed.** Every mutation takes the raw
  `Bytes` request body and deserializes it *after* `require_admin` succeeds, so
  an unauthenticated caller always gets `401` with the API's JSON envelope and
  learns nothing about the payload shape. Extracting `Json<T>` first (the shape
  before the fix) answered `422` and echoed the deserializer's message.
- **A conflict is refused, not overwritten.** Enabling DHCP on an interface
  where another server already answers is a `409`, never a silent takeover.
- **Validation precedes persistence.** `validate_config` and the conflict probe
  both run before anything is written, so a refused configuration leaves the
  previous one intact and the daemon untouched.
- **Turning both servers off stops the daemon.** "Disabled" and "stopped" are
  the same state; there is no way to leave `dnsmasq` running against a
  configuration the admin has turned off.
- **The address range must be inside one subnet.** Start < end, both inside the
  mask. A range that spans two subnets is refused by name.
- **Turning the server on is confirmed; nothing else is.** It is the one action
  that can hand out wrong addresses on a live LAN. The confirmation is a
  focus-trapping `Modal`, and it lists what must be true before continuing.
- **Every subprocess has a deadline.** Probes (`dnsmasq --version`,
  `systemctl is-active`) run under `PROBE_TIMEOUT`, the daemon start/stop under
  `DNSMASQ_TIMEOUT`, the conflict probe under `DHCP_DISCOVER_TIMEOUT` and the
  root-owned drop-in write under `CONFIG_WRITE_TIMEOUT` — all through
  `utils::process::run_timed_async*`, which the tree-wide guard
  (`utils::process::tests::every_subprocess_of_the_tree_is_bounded`) enforces.

## Operations the admin mutates, and what each does

| Action | Effect on the host |
|--------|--------------------|
| Save with DHCP and/or TFTP on | writes `/etc/dnsmasq.d/homevault.conf`, restarts `dnsmasq`, verifies it is running |
| Save with both off | stops `dnsmasq` |
| Start | starts `dnsmasq` from the saved config; refuses if both switches are off |
| Stop | stops `dnsmasq` |

A `dnsmasq` that reports success but is not actually running afterwards is an
error, not a success — the start path re-reads the daemon state.

## Tests

| Suite | What it covers |
|-------|----------------|
| `src/dhcp_server.rs` (`#[cfg(test)]`) | range order, subnet containment, MAC format, TFTP root must be absolute |
| `tests/dhcp_server_integration.rs` | validation matrix and the service surface |
| `tests/phase4_auth_integration.rs` | `401` without a session, `403` for a member on every mutation, admin never refused |
| `frontend/src/lib/components/settings/DhcpServerCard.svelte` | covered by `npm run check`; E2E in `tests/e2e/settings/` |

## Related

- [Network interfaces and diagnostics](./network.md)
- [Firewall](./firewall.md) — `dnsmasq` needs UDP 67/68 and, for PXE, UDP 69
- [mDNS](./mdns.md) — the other discovery mechanism the appliance offers
