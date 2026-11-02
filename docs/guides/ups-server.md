# UPS server mode

**Purpose:** expose the appliance as a NUT (Network UPS Tools) server, so other
machines on the LAN can watch the same battery and shut down with it. The
complement of the UPS *client* card, which watches a UPS from the appliance's
point of view.

**Who can use it:** any signed-in account may read the configuration and the
daemon state. Saving, starting, stopping and **issuing the shutdown** are
**admin-only**. The shutdown route in particular ends the machine; a denial
there is the difference between an appliance and a LAN-wide remote power-off
button.

## Where it lives

- **Screen:** Settings → Hardware → **UPS server mode** →
  `frontend/src/lib/components/settings/UpsServerCard.svelte`.
- **Backend:** `src/ups_server.rs` (config validation, `upsd.conf`/
  `upsd.users` generation, daemon lifecycle, per-client ACL, orderly shutdown)
  and `src/handlers/ups_server.rs` (HTTP surface).
- **Storage:** `{DATA_DIRECTORY}/data/ups_server.json`.
- **Dependencies:** `upsd` (package **nut**, which also provides `upsc`) and
  `systemctl`. `src/dependencies.rs` has a `DependencySpec` for each;
  `installer/install.sh` offers `nut` unconditionally.

## Endpoints

| Method | Path | Guard |
|--------|------|-------|
| `GET`  | `/api/power/ups-server` | `require_auth` |
| `PUT`  | `/api/power/ups-server` | `require_admin` — save **and apply** |
| `POST` | `/api/power/ups-server/start` | `require_admin` |
| `POST` | `/api/power/ups-server/stop` | `require_admin` |
| `POST` | `/api/power/ups-server/shutdown` | `require_admin` **+ explicit `confirm: true`** |

## Rules

- **The guard runs before the body is parsed.** Every mutation takes raw
  `Bytes` and deserializes after `require_admin`, so an unauthenticated caller
  gets `401` and cannot probe the payload.
- **A shutdown must be confirmed.** A body without `confirm: true` is a `400`
  naming the rule, never a silent no-op — and never a shutdown.
- **The listener is never `0.0.0.0` by default.** `bind_interface` is required
  and an empty value is refused; the address is derived from that interface.
- **Reserved ports are refused.** `0`, `22`, `80`, `443` and `8443` are
  rejected: the last one is HomeVault's own alternative HTTPS listener, and
  `upsd` taking it would make the dashboard unreachable.
- **At least one client is required when enabled.** An open NUT server with no
  allowlist is not a configuration this appliance offers.
- **Config files are written as root, 0600.** `sudo tee` under
  `CONFIG_WRITE_TIMEOUT`, followed by `sudo chmod 0600`.

## Orderly shutdown

The policy names the stages to run, in this order:

| Stage | What it does | Status in this build |
|-------|--------------|----------------------|
| `stop_containers` | `docker ps -q`, then `docker stop -t 30 <ids>` | **implemented** |
| `shutdown_vms` | graceful VM shutdown | **not available** — reported `skipped` |
| `flush_restic` | drain pending backup work | **not available** — reported `skipped` |
| `stop_shares` | `systemctl stop smbd nmbd nfs-server` | **implemented** |
| `final_poweroff` | `sudo poweroff` | **implemented**, always runs |

### Why two stages are "not available" rather than "done"

`shutdown_vms` and `flush_restic` had no implementation and returned `Ok(())`.
That reported success for work that never happened, which is exactly how an
orderly shutdown silently becomes an abrupt one: the operator reads "done",
power is cut, and the VMs die with the disk.

They now return `StageOutcome::Skipped(reason)`, and the HTTP payload carries a
`skipped: true` field beside `success`. The response's top-level `success` is
`false` when any enabled stage was skipped — "did not happen" is never reported
as "done". The card says the same thing in the shutdown-policy section.

### The `docker stop` fix

`docker stop -t 30 "$(docker ps -q)"` was passed as **argv**, not through a
shell. Docker received the literal string `$(docker ps -q)` as a container
name, stopped nothing, and still exited 0. The stage now runs `docker ps -q`
first, splits its output into ids, and passes them as real arguments — no shell
anywhere.

## Tests

| Suite | What it covers |
|-------|----------------|
| `src/ups_server.rs` (`#[cfg(test)]`) | client validation (name, address, username), reserved ports, `max_clients` bounds, **skipped stages never report success**, unknown stage is an error |
| `src/handlers/ups_server.rs` (`#[cfg(test)]`) | config round-trip, `skipped` present in the HTTP payload |
| `tests/ups_server_integration.rs` | availability, config validation, reserved ports |
| `tests/phase4_auth_integration.rs` | `401` without a session, `403` for a member on every mutation, admin refused on an unconfirmed shutdown, admin never refused elsewhere |

## Related

- [Power](./power.md) — the UPS *client* card, CPU governor, disk spindown
- [Server control](./server-control.md) — the manual restart/shutdown actions
- [Scheduled tasks](./scheduled-tasks.md) — where a power-off is scheduled
