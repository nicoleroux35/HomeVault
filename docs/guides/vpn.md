# VPN (WireGuard / OpenVPN)

**Purpose:** Manage WireGuard and OpenVPN connections. **Every** VPN endpoint
requires an **admin** session — there are no read-only `auth` endpoints (the whole
surface is privileged: it writes `/etc` and brings up interfaces as root).

## Backend

Router built by `src/handlers/vpn.rs::create_vpn_router`, nested at `/api/vpn`.
Service in `src/vpn.rs`.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /vpn/capabilities` | admin | which backends are installed — payload is `data.capabilities` |
| `GET /vpn/connections` | admin | list connections — payload is `data.connections` |
| `GET /vpn/connections/{id}` | admin | connection detail — payload is `data.connection` |
| `POST /vpn/wireguard` | admin | create (`{ name, address, peers[], ... }`) |
| `POST /vpn/wireguard/keygen` | admin | generate keypair |
| `POST /vpn/wireguard/{name}/start\|stop` | admin | toggle |
| `DELETE /vpn/wireguard/{name}` | admin | delete |
| `POST /vpn/openvpn` | admin | create (`{ name, config_content, ... }`) |
| `POST /vpn/openvpn/{name}/start\|stop` | admin | toggle |
| `DELETE /vpn/openvpn/{name}` | admin | delete |

## Frontend

- `frontend/src/routes/VPN.svelte` — reachable from the desktop icon grid and
  launcher as the `vpn` entry in `Desktop.svelte`'s `apps` array. (The page and
  its backend were both fully built before, but the page had never been added
  to that array, so it was unreachable from any UI — see
  [Apps Center](./apps-center.md).)
- `frontend/src/lib/services/api.ts`: `getVpnCapabilities`, `listVpnConnections`,
  `getVpnConnection`, `createWireGuardConnection`, `generateWireGuardKeys`,
  `startWireGuardConnection`, `stopWireGuardConnection`,
  `deleteWireGuardConnection`, `createOpenVPNConnection`,
  `startOpenVPNConnection`, `stopOpenVPNConnection`, `deleteOpenVPNConnection`.

## Invariants

- `handle_vpn_capabilities`/`handle_vpn_list`/`handle_vpn_get` must keep
  returning the `ApiSuccess{success, message?, data}` envelope, not a bare
  struct — a past bug had them return the raw payload directly, so
  `VPN.svelte`'s `response.success && response.data` check was always
  `false`, permanently showing "No VPN Support" even with WireGuard
  genuinely installed. Rust doesn't catch this shape mismatch at compile
  time and the frontend fails silently (no error, just empty state), so it's
  easy to reintroduce by adding a new read endpoint that returns a bare
  struct instead of reusing `ApiSuccess`.

## Related

- [Network](./network.md), [Apps Center](./apps-center.md), [API Contract](../API.md)
