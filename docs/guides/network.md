# Network

**Purpose:** Ethernet interface configuration, WiFi scan/connect, diagnostics, and
connection summary.

## Persistance (2026-10-20)

`apply_interface_config` changes the live state only (`ip`, `dhclient`,
`/proc/sys`); `persist_interface_config` (`src/network.rs`) is called by the
config handler afterwards to make the change survive a reboot. Two backends,
probed in order:

1. **netplan** — `<base>/99-homevault-<name>.yaml`. The `99-` prefix and the
   `ethernets.<name>` key keep the file scoped to the one interface the admin
   touched; the `renderer` is **mirrored** from the existing netplan files
   (lexicographically last declared value wins, like netplan's own merge), so
   a NetworkManager-managed host is never silently handed to networkd. Static
   v4 + static v6 share one `addresses:` list and one `routes:` table
   (duplicate keys are a netplan parse error). The file is written mode 600
   over `sudo tee`.
2. **ifupdown** — `/etc/network/interfaces.d/homevault-<name>`, only when the
   include directory exists **and** `/etc/network/interfaces` sources it.

With neither backend, the request still succeeds (the live state is applied)
but the message becomes `"(temporary — persistence unavailable)"` and the
frontend shows the `network.temp_config` warning toast. A persistence failure
must never fail the request; it is logged as a warning. The YAML/stanza
rendering is pure and unit-tested (`test_netplan_yaml_*`,
`test_ifupdown_stanza_*`); no secrets are ever written (the Wi-Fi PSK stays in
wpa_supplicant's store).

## Apply, verify, roll back (2026-10-21)

Since 2026-10-21, persisting is followed by **actually applying** the persisted
file (`apply_persisted_config`, called by the config handler right after
`persist_interface_config`), so runtime and boot state can no longer drift by a
re-apply cycle and the admin sees in the same session that the file is
loadable and equivalent to what is on the wire:

1. **Capture** — before anything is touched, the handler reads
   `get_interface_config(name)`: that pre-change state is the rollback target.
2. **Live apply** — unchanged (`ip`, `dhclient`, `/proc/sys`); on failure
   nothing has been touched yet and the raw error is returned.
3. **Persist** — as above; `persist_interface_config` now returns a
   `PersistedChange` carrying which backend took the write plus the
   pre-change snapshot of our own netplan file (read back via `sudo cat` —
   after the first write the file is root-owned mode 600).
4. **Reconcile (netplan backend only)** — `netplan apply` re-asserts the exact
   settings the live step already put in place, then the wire is **verified**
   with a 10 s polling grace (250 ms steps, `VERIFY_GRACE`/`VERIFY_STEP`):
   static v4 address present, default route `via` the configured gateway,
   static v6 `addr/prefix` present, interface still up. The verdict is a pure
   function (`check_live_state_matches`, unit-tested) fed by live reads.
5. **Roll back on any failure** — both halves: restore the previous netplan
   file (or delete ours when none existed — `sudo rm -f` scoped to
   `/etc/netplan/99-homevault-*.yaml`), run `netplan apply` again, then
   re-apply the previous live configuration via `apply_interface_config` —
   that last step is what keeps the operator's own session alive. The API
   answers `success:false` with the failure detail; `Network.svelte`
   reloads the interfaces so the cards reflect the rolled-back wire.

Design decisions worth keeping:

- **Ifupdown stays write-only.** Its stanza is applied by `ifup` at boot; an
  `ifdown`/`ifup` pair would tear the interface down under the operator — the
  exact concern the rollback pattern closes for netplan. `ifupdown` changes
  there answering `(persisted)` without a reconcile.
- **Renderer choice is never invented**: only our own `99-homevault-*` files
  are written/restored; on a failed reconcile the distro's files are untouched.
- **A DHCP change needs *a* lease after the apply** (any global v4 address) —
  the wire must converge for the change to count as applied.
- The `netplan apply`, `chmod 600 /etc/netplan/99-homevault-*.yaml`,
  `rm -f /etc/netplan/99-homevault-*.yaml` and
  `cat /etc/netplan/99-homevault-*.yaml` shapes are scoped in the sudoers
  file (`src/setup.rs::configure_sudoers`) for the legacy non-root service
  mode; the root systemd service routes through sudo transparently.
- `netplan.io` is an optional dependency (`src/dependencies.rs`,
  `installer/install.sh`): Ubuntu always ships it, Debian hosts without it
  fall through to ifupdown or stay temporary-only.

## Backend
Router built by `src/handlers/network.rs::create_network_router` and nested at
`/api/network` (`src/web/mod.rs:1069`).

Wi-Fi operations shell out to `wpa_cli` (sudoers-granted, see
`src/setup.rs`), which ships in the **`wpasupplicant`** package — installed by
`installer/install.sh` and re-checked in its Step 4. An Ethernet-only host is
unaffected: the Wi-Fi endpoints report the tool as unavailable.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /network/interfaces` | auth | list interfaces |
| `GET /network/interfaces/{name}` | auth | interface detail |
| `GET /network/interfaces/{name}/config` | auth | IP/DNS/gateway config |
| `PUT /network/interfaces/{name}/config` | admin | change config — persisted **and applied** (apply-verify-rollback, see above); a failed reconcile answers `success:false` after the wire was rolled back to the pre-change state |
| `PUT /network/interfaces/{name}/state` | admin | bring up/down (`{ up }`) |
| `GET /network/wifi/scan` | auth | scan APs |
| `GET /network/wifi/status` | auth | WiFi status |
| `POST /network/wifi/connect` | admin | connect (`{ ssid, password, security }`) |
| `POST /network/wifi/disconnect` | admin | disconnect (`{ ssid }`) |
| `GET /network/diagnostics` | auth | gateway/DNS/internet checks |
| `GET /network/summary` | auth | connection summary |
| `GET /network/public-ip` | auth | public IP (geo lookup) |
| `GET /network/services` | auth | managed network services list — not currently called by `Network.svelte` or `api.ts` |

## Frontend

- `frontend/src/routes/Network.svelte`.
- `frontend/src/lib/services/api.ts`: `getNetworkInterfaces`,
  `getNetworkInterface`, `getNetworkInterfaceConfig`,
  `updateNetworkInterfaceConfig`, `setNetworkInterfaceState`, `scanWifiNetworks`,
  `getWifiStatus`, `connectWifi`, `disconnectWifi`, `getNetworkDiagnostics`,
  `getNetworkSummary`, `getPublicIp`.

## Invariants

- All 13 endpoints require a session (a past audit found them wide open). Read
  endpoints use `require_auth`; config/state/WiFi mutations use `require_admin` —
  do not collapse that split to "no check".
- Toast messages use translation keys (a past bug hardcoded English strings);
  diagnostic labels are not double-prefixed (a past bug produced "Gateway Gateway
  reachable").
- The `[network]` section of `fr.toml` must stay fully translated (a ~50-key gap
  existed; DE/ES were correct).
- The verify verdict `check_live_state_matches` is pure and unit-tested; the
  live netplan path itself needs a real host (see `TESTING.md` — "remaining to
  test live"). Never run `netplan apply` outside the apply-verify-rollback
  pattern.
- **Editable since 2026-09-12** — `CLAUDE.md`'s frozen-area registry is gone, so
  `Network.svelte`, the network handlers and the `network.*` i18n section are
  open to edits. The invariants above stay binding: a change keeps the Rust
  integration + Playwright E2E suites green.

## Related

- [API Contract](../API.md), [VPN](./vpn.md)
