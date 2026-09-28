# Network

**Purpose:** Ethernet interface configuration, WiFi scan/connect, diagnostics, and
connection summary.

## Backend

Router built by `src/handlers/network.rs::create_network_router` and nested at
`/api/network` (`src/web/mod.rs:1069`).

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
