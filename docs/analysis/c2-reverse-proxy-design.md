# C2 — Generic reverse proxy: design note

Status: implemented (2026-11-06). Guide:
[`docs/guides/reverse-proxy.md`](../guides/reverse-proxy.md).

This note records the two decisions the tasklist asked for before the first
line of code, and the security invariants the implementation must keep. It is
the reference for reviewing the feature: if the code and this note disagree,
one of them is a defect.

---

## 1. What the item is

A user-defined map from a **host name** (and optional path prefix) to an
**upstream** `host:port`, so a LAN service that has no friendly URL can be
reached as `ha.home.lan` instead of `192.168.1.50:8123`. It is *not* a second
implementation of the per-app catalog proxy (`src/app_proxy/`), and it is not a
public ingress: it listens on the LAN like every other HomeVault service.

Non-goals for this version:

- no wildcard hosts, no regex paths, no URL rewriting;
- no TLS **termination** on the proxy listener (HTTP only, like the per-app
  listener); the upstream may itself be HTTPS (`tls: true`);
- no configuration import from nginx/Caddy/Traefik.

## 2. Decision 1 — one dedicated listener, host-routed

The rules are served by **one** additional listener, not by the main UI
listener and not by one listener per rule.

| Option | Verdict |
|---|---|
| Route by `Host` on the existing main listener | Rejected. The dashboard would share a router with user-defined upstreams; one routing mistake exposes the API under a user-chosen name, and the main listener's TLS/session plumbing would have to learn about rules. |
| One listener per rule (like the per-app proxy) | Rejected. It cannot express `host`, so it would not be a *generic* proxy; the per-app proxy already does that job. |
| **One listener for all rules, selected by `Host` + longest path prefix** | **Chosen.** One bind, one code path, rules are data. |

- Default port **80** (`HOMEVAULT_REVERSE_PROXY_PORT` overrides it). A failed
  bind (port taken, missing privilege) is **not fatal**: the CRUD API and the
  status report say the listener is down, with the reason. The rest of
  HomeVault keeps running.
- The listener is **plain HTTP**. `tls` in a rule means the *upstream* is
  HTTPS. Browser-facing TLS termination for these names is out of scope; a
  future item can front it with the appliance certificate.
- Matching: normalise the request `Host` (port stripped, lower-cased), require
  an exact rule host, then keep the enabled rule whose `path` is the longest
  segment-boundary prefix of the request path (`""` = whole host). No match is
  a `404`; a match forwards the **full path unchanged**.

## 3. Decision 2 — the auth gate is a cross-host ticket exchange

Reusing the per-app gate as-is is impossible: `hv_app_session` is a
**host-scoped cookie**, and a generic rule deliberately uses a *different*
host name than the dashboard. A cookie minted on the dashboard host is never
sent to `ha.home.lan`, so the per-app round-trip would loop forever.

The gate therefore keeps the *credential model* (a narrow, rule-scoped session
from `APP_AUTH`, never `hv_session`) and changes only the hand-off:

1. A gated request without a valid `hv_app_session` is `302`-ed to the **main
   origin** — whose host is resolved by `app_proxy::origin` from server state,
   never from the request's `Host` — at
   `GET /proxy-auth?rule=<id>&next=<path>`.
2. `/proxy-auth` validates the real `hv_session` cookie, confirms the rule
   still exists and is enabled, mints a **one-time ticket** (120 s, single use,
   rule-bound), and redirects to
   `http://<rule.host>:<proxy_port>/__hv_proxy_auth?rule=…&ticket=…&next=…`.
   The target host comes from the **stored rule**, not from a header.
3. The listener's `__hv_proxy_auth` endpoint redeems the ticket, mints an
   `APP_AUTH` session bound to `proxy:<id>`, sets `hv_app_session` **on the
   rule's host**, and redirects to `next`.
4. Later requests present that cookie, which the listener validates exactly as
   the per-app gate does.

Properties this buys:

- the browser never sends the API session to a user-defined upstream;
- a ticket in a URL is single-use, expires in 120 s, and is bound to one rule,
  so a leaked URL cannot be replayed elsewhere;
- redirects are built from stored state, so a forged `Host` cannot aim a
  cookie-bearing redirect at another origin;
- `next` must be a relative path (`/…`, no `//`, no scheme, no backslash), so
  the final redirect cannot leave the rule's origin;
- revocation is per rule (`APP_AUTH::revoke_app`), on delete, disable, or
  edit — never household-wide.

Rules with `auth_required: false` skip the gate. The card defaults it to on and
says plainly that the upstream is then reachable by anyone who can reach the
appliance.

## 4. Security invariants (the review checklist)

The per-app proxy's safety rests on "never trust a forged header"
(`docs/guides/apps-center.md`). A generic proxy is the obvious place to lose
it, so these are structural:

1. **Inbound forwarding headers are discarded, not merged.** `Forwarded`,
   `X-Forwarded-For`, `X-Forwarded-Host`, `X-Forwarded-Proto`,
   `X-Forwarded-Port`, `X-Real-IP`, `X-Original-Forwarded-For`,
   `X-Rewrite-URL` and the method-override headers are removed from the
   inbound request. `X-Forwarded-For` and `X-Forwarded-Proto` are then set
   from the **peer socket** and the listener's own scheme.
2. **HomeVault cookies never reach an upstream.** `hv_session` and
   `hv_app_session` are stripped unconditionally, gate on or off.
3. **The upstream is an intranet address.** Syntax is `host:port` (IPv6
   bracketed), no URL. At save time *and* per connection, the name is resolved
   and **every** address must be loopback, private, link-local or ULA. A
   public address is a `400` naming the address; a name that resolves to a
   mix is refused. This blocks the obvious SSRF pivot and keeps the feature
   inside the LAN target.
4. **No self-loop.** The listener refuses to forward to its own port, or to
   the main UI port on a local address, so a rule cannot recursively proxy
   HomeVault into itself.
5. **The listener is not an auth boundary replacement.** Disabling the gate is
   an explicit admin choice, recorded per rule; the upstream's own auth still
   applies.
6. **No secret leaves the API.** Upstream addresses and paths are shown to any
   signed-in user; mutations are admin-only; tickets and sessions are never
   returned by the rules API.

## 5. Data model

One table, `reverse_proxy_rules` (migration `076_reverse_proxy.sql`):

| Column | Meaning |
|---|---|
| `host` | Normalised host name / IP literal, `UNIQUE(host, path)` with `NOCASE` |
| `path` | `""` or `"/prefix"`; longest segment prefix wins |
| `upstream` | Canonical `host:port` |
| `tls` | Upstream scheme is HTTPS |
| `auth_required` | Session gate on (default on) |
| `enabled` | Rule is live; disabled rules are never matched and their sessions are revoked |

Rules are loaded into an in-memory snapshot at startup and after every
mutation, so a request never queries SQLite on the hot path.

## 6. Failure modes and what the user sees

| Failure | Answer |
|---|---|
| Listener cannot bind | Status `listening: false` + the OS reason; CRUD still works |
| No rule matches | `404` plain text naming the host |
| Upstream down / TLS handshake fails | `502`, never a hang (10 s connect timeout) |
| Redirect to `/proxy-auth` but the listener is down | `503` naming the port |
| Ticket invalid/expired/replayed | `401`, the request is not forwarded |
| Upstream resolves to a public address | Save is `400`; if DNS changes later, the connection is refused at request time with `502` |
