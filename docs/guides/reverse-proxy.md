# Reverse proxy (generic)

HomeVault can publish a service that runs elsewhere on your network under a
name of your own, so `ha.home.lan` reaches `192.168.1.50:8123` without anyone
remembering the port. It is one listener for every rule, selected by the host
name the browser asks for. It is **not** the per-app proxy that the Docker
catalog creates (see [`apps-center.md`](./apps-center.md)); that one is per
installed app and always on loopback.

Settings → Security → **Reverse proxy** (`/api/proxy/rules`).

---

## What a rule is

| Field | Meaning |
|---|---|
| **Host name** | The exact name devices type, e.g. `ha.home.lan`. No port, no scheme, no wildcard. |
| **Path prefix** | `/` for the whole host, or a prefix such as `/api`. The longest matching prefix wins, on a segment boundary (`/api` matches `/api` and `/api/x`, never `/apix`). The upstream receives the **full path unchanged**. |
| **Service address** | `host:port` — an IPv4 literal, a DNS name, or a bracketed IPv6 literal. It must resolve to a **private, loopback, link-local or ULA** address. |
| **HTTPS service** | The upstream speaks HTTPS. The browser still reaches the appliance over plain HTTP; only the hop to the service is encrypted. |
| **Require a HomeVault sign-in** | On by default. A browser without a rule-scoped session is sent through the dashboard to sign in first. |
| **Active** | A disabled rule is kept but never answers, and its sessions are revoked. |

Rules are limited to 100 per appliance, and `(host, path)` is unique.

## The listener

One listener serves every rule, by default on **port 80**. Override it with
`HOMEVAULT_REVERSE_PROXY_PORT` (then restart HomeVault).

A bind failure is **not** fatal: the dashboard, the rules and the rest of the
appliance keep working, and the card shows the OS reason above the list. The
first thing the card reports is therefore whether the listener is up, on which
port, and why not.

For a name to work from another device, it must already resolve to the
appliance. The **Local DNS server** card (phase C1) is the usual way: add an
`A` record for the name, then create the rule here. The card says so at the
bottom.

## The sign-in gate

The gate never lets the browser hand the API session to a service. The
credential is a **rule-scoped session**, minted after the dashboard validates
the real one:

1. A gated request without a valid session is redirected to the dashboard at
   `GET /proxy-auth?rule=<id>&next=<path>`. The dashboard host is resolved by
   the server (`app_proxy::origin`), never taken from the request's `Host`
   header.
2. `/proxy-auth` validates the `hv_session` cookie, confirms the rule is still
   enabled, mints a **one-time ticket** (120 s, single use, bound to that rule)
   and redirects to `http://<rule.host>:<port>/__hv_proxy_auth?…`.
3. The listener redeems the ticket, mints an `hv_app_session` for that rule
   **on the rule's host** and redirects to the original path. Later requests
   carry that cookie.

Why the ticket: the per-app proxy's cookie round-trip relies on the proxy and
the dashboard sharing a host name. A generic rule deliberately uses a different
name, and cookies are host-scoped, so the dashboard cannot set the session
cookie for `ha.home.lan` directly. The single-use ticket is the narrowest
bridge between the two origins.

Turning the gate off is an explicit admin choice. The card says what it means:
anyone who can reach the appliance can reach the service. The service's own
authentication still applies.

## What the proxy never does

- **Never trusts a forged header.** `Forwarded`, every `X-Forwarded-*`,
  `X-Real-IP`, `X-Rewrite-URL` and the method-override headers are discarded
  from the inbound request. `X-Forwarded-For`, `X-Real-IP` and
  `X-Forwarded-Proto` are then set from the peer socket and the listener's own
  scheme. A header the proxy does not own (an application's own) is preserved.
- **Never forwards a HomeVault cookie.** `hv_session` and `hv_app_session` are
  stripped unconditionally, gate on or off; the service's own cookies stay.
- **Never reaches a public address.** The upstream is resolved at save time
  *and* on every connection; every address must be intranet. A name with a
  mixed answer, or one that later starts resolving publicly, is refused.
- **Never loops into itself.** A rule cannot point at the proxy's own port or
  at the dashboard port on a local address.
- **Never rewrites the response `Location`.** An upstream that redirects
  continues to control its own redirects; only the request side is normalised.

WebSocket upgrades are relayed (`Upgrade` + `101` + spliced streams), so a
service that uses one works through a rule.

## Failure answers

| Situation | What happens |
|---|---|
| No rule matches the host | `404` naming the host |
| Service down | `502` with the OS reason; the dial times out after 10 s |
| Upstream TLS certificate not trusted | `502`, handshake refused (no per-rule CA pinning in this version) |
| Listener down when `/proxy-auth` runs | `503` naming the port and the bind error |
| Ticket expired, reused or for another rule | `401`, nothing is forwarded |

## Tests and verification

| Level | File |
|---|---|
| Unit | `src/reverse_proxy/mod.rs` — host/path/upstream grammar, prefix selection, private-range policy, loop refusal |
| Unit | `src/reverse_proxy/proxy.rs` — `next` validation, redirect builders, header sanitisation, cookie stripping |
| Unit | `src/reverse_proxy/ticket.rs` — single use, rule binding, revocation |
| Integration | `tests/reverse_proxy_integration.rs` — guards, CRUD, validation, `/proxy-auth`, and a real listener against a loopback upstream (headers, cookie stripping, longest prefix, ticket redemption, public-address refusal) |
| E2E | `tests/e2e/settings/reverse-proxy.spec.ts` |

**Never exercised live:** a browser reaching a rule from a second device, and
a real TLS upstream certificate. Both belong on a disposable VM; every
listener test here uses loopback.
