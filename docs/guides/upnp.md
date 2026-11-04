# Router port forwarding (UPnP-IGD)

HomeVault can list and edit the port mappings a **UPnP-IGD** router exposes, so
a service on your network can be reached from the internet without opening the
router's own web interface. It talks to the router through the `upnpc` client
that ships with **miniupnpc**.

Settings → Security → **Router port forwarding**
(`GET /api/network/upnp`, `POST|DELETE /api/network/upnp/mappings`).

> **The feature never turns UPnP on.** If UPnP is disabled on the router (the
> safer default on many models), nothing answers and the card says so. Only an
> administrator can change a mapping, and every change is a confirmed action.

---

## What the card shows

| Field | Meaning |
|---|---|
| **External address** | The router's public address, as the router reports it. Unknown when the router does not answer. |
| **Router** | The IGD control URL used for the conversation. |
| **This appliance** | The local address `upnpc` selected for the discovery. |
| **Current mappings** | Every mapping the router holds — not only the ones HomeVault added — with protocol, external port, internal client and lease. |

Discovery is **read-only**: opening the screen changes nothing on the router.

## Adding a mapping

| Field | Rule |
|---|---|
| **Protocol** | `TCP` or `UDP`. |
| **External port** | `1..=65535` — the port the internet connects to. |
| **Device address** | An IPv4 or IPv6 **literal** on the local network. Loopback, public, unspecified and host names are refused. |
| **Device port** | `1..=65535` — the port the service listens on. |
| **Description** | Printable ASCII, no quotes or backslashes, 64 characters including the `HomeVault` prefix. Empty becomes `HomeVault`. |
| **Lease** | 1 hour (default), 6 hours, 1 day, 7 days, or permanent. |

Submitting requires the confirmation checkbox; the server also refuses a body
without `confirmed: true`, so a script cannot open a port by accident.

The description is prefixed with `HomeVault` (or `HomeVault: <text>`) so the
mapping is recognisable in the router's own table and can be attributed.

## Removing a mapping

The **Remove** button names the mapping and its internal target in a
confirmation dialog, then calls `upnpc -d <port> <protocol>`. Any mapping in
the list can be removed, including one that was added outside HomeVault — the
list is the router's, not a local copy.

Removal requires `confirmed: true` on the API too. A mapping is identified by
its `(protocol, external port)` pair, exactly what the router uses; there is no
wildcard and no "remove everything".

## Security and failure answers

- **No shell.** Arguments are validated and passed one by one to `upnpc`; a
  description cannot inject a flag, a quote or a newline.
- **No credential is stored.** IGD discovery is unauthenticated on the LAN; the
  appliance keeps no router password.
- **LAN targets only.** The internal client must be private, link-local or
  ULA. Public and loopback targets are refused because they either cannot work
  or would point the router at the wrong machine.
- **Bounded calls.** `upnpc` runs with an 8 s deadline; a silent gateway
  surfaces as a sentence, not a spinning card.
- **Absent client.** A read answers `available: false` and the card names the
  `miniupnpc` package; a mutation is a `503` naming the package.
- **Router refusal.** A failed write is a `502` whose message is the router's
  or `upnpc`'s own sentence (for example a port already mapped).
- **Reversible.** A write that succeeds but whose follow-up list fails is
  reported as success with the listing error, never as a failed write — the
  mapping is really there and the list is one Refresh away.

## Tests and verification

| Level | File |
|---|---|
| Unit | `src/upnp.rs` — listing parser (gateway, external IP, description with spaces, permanent lease, malformed row), internal-client policy, port/lease bounds, description breakouts, exact `upnpc` argv, confirmation |
| Unit | `src/handlers/upnp.rs` — lease label |
| Integration | `tests/upnp_integration.rs` — `401`/`403` matrix, guard before body parsing, every refusal that happens before `upnpc` runs, `GET` capability shape |
| E2E | `tests/e2e/settings/upnp.spec.ts` — every call intercepted, so no real router is touched |

**Never exercised live:** a real router. The suite here has no IGD, and opening
a port on the machine hosting the tests would change its network face. The
listing parser is tested against the `upnpc -l` output shape; a manual pass on
a disposable LAN with a UPnP-enabled router is the remaining check.
