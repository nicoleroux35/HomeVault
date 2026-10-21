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
must never fail the request; it is logged as a warning. The persisted config
is **written, never applied** — running `netplan apply` right after the live
mutations would tear the interface down under the admin's own session — runtime
and boot state may therefore differ for one re-apply cycle. The YAML/stanza
rendering is pure and unit-tested (`test_netplan_yaml_*`, `test_ifupdown_stanza_*`);
no secrets are ever written (the Wi-Fi PSK stays in wpa_supplicant's store).

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
| `PUT /network/interfaces/{name}/config` | admin | change config |
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
- **Editable since 2026-09-12** — `CLAUDE.md`'s frozen-area registry is gone, so
  `Network.svelte`, the network handlers and the `network.*` i18n section are
  open to edits. The invariants above stay binding: a change keeps the Rust
  integration + Playwright E2E suites green.

## Related

- [API Contract](../API.md), [VPN](./vpn.md)
