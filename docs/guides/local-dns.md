# Local DNS server

**Purpose:** let the appliance answer DNS for the local network — the names
that live inside the house (`nas.lan`, `printer.lan`, a whole redirected
domain) — instead of every device depending on the router's resolver. Names it
does not know are forwarded to the upstream resolvers the admin lists.

**Who can use it:** any signed-in account may read the configuration and the
live state. Saving is **admin-only** — a wrong record here breaks name
resolution for every device on the LAN, not just for this screen.

## Where it lives

- **Screen:** Settings → Security → **Local DNS server** →
  `frontend/src/lib/components/settings/LocalDnsCard.svelte`, right after the
  DHCP server card (they are the same `dnsmasq`).
- **Backend:** `src/local_dns.rs` (validation, the rendered DNS section,
  persistence, interface candidates, `resolv.conf` parsing) and
  `src/handlers/local_dns.rs` (HTTP surface).
- **Composer:** `src/dhcp_server.rs` → `DhcpServerService::render_conf` renders
  the **one** drop-in both halves share, and
  `DhcpServerService::reconcile_daemon` is the single gate that starts,
  restarts or stops the daemon.
- **Storage:** `{DATA_DIRECTORY}/data/local_dns.json` — a single document, no
  migration (its sibling is `dhcp_server.json`).
- **Host file:** `/etc/dnsmasq.d/homevault.conf`, root-owned, written through
  `sudo tee` (no sudoers file is generated — see `CLAUDE.md` → Deployment
  model).
- **Dependency:** `dnsmasq` (`src/dependencies.rs`, offered by
  `installer/install.sh` as an optional package). When it is absent the card
  still loads and names the missing package.

## Endpoints

| Method | Path | Guard |
|--------|------|-------|
| `GET`  | `/api/network/dns-server` | `require_auth` |
| `PUT`  | `/api/network/dns-server` | `require_admin` — save **and apply** |

`GET` answers `{ config, state, available, interfaces[], system_forwarders[],
dhcp_enabled }`; `state` carries `running` (the shared daemon is up) and
`serving` (running **and** this feature is on).

## One daemon, two features — the rule this module exists to keep

`dnsmasq` serves DHCP, TFTP and DNS at once. Before C1 the DHCP card stopped the
daemon whenever it was saved with both of its switches off; once DNS rides on the
same process that would have been a silent outage of the LAN's resolver. So:

- **One drop-in.** Both halves are rendered into
  `/etc/dnsmasq.d/homevault.conf` by `render_conf`. Neither feature writes its
  own file: two files would race for the same directives and whichever was
  written last would silently win.
- **One listening policy.** `interface=` is emitted once per served interface
  (DHCP's and DNS's, deduplicated) followed by a single `bind-interfaces`, so the
  daemon answers exactly those interfaces and nothing else.
- **One gate.** Every save on either card calls `reconcile_daemon`: any of the
  three switches on → compose + `systemctl restart` + verify the unit is still
  up; all three off → remove the drop-in and stop the daemon. A save on the
  DHCP card therefore never stops a running DNS server, and a save on the DNS
  card never stops a running DHCP server.
- **A refused step is undone.** The handler persists, applies, and on a host
  error restores the previous document and re-applies it, so "saved" and
  "serving" cannot disagree silently.
- **The DHCP card cannot stop the daemon behind DNS's back.**
  `POST /api/network/dhcp-server/stop` answers **`409`** while the DNS server is
  on and names the card that owns the decision.

## Rules the configuration keeps

- **Off by default.** An appliance that answers DNS for a LAN it was never asked
  to answer is a way to break a working network.
- **Enabling is confirmed.** `confirmed: true` plus an interface plus at least
  one upstream forwarder; the confirmation is a focus-trapping `Modal` listing
  what must be true first.
- **Nothing leaks upstream by accident.**
  - `no-resolv` — the forwarders listed here are the only ones used; the
    appliance's own `/etc/resolv.conf` cannot change what the LAN resolves.
  - `domain-needed` — a bare name (no dot) is never forwarded.
  - `bogus-priv` — reverse lookups of private ranges never leave the network.
    Without it a LAN's device names would be resolved by, and visible to, the
    ISP's resolver.
  - `local-service` — only a resolver on a directly attached subnet is answered,
    never one that reached the appliance from the internet.
- **Loopback is not a forwarder.** `127.0.0.1` is refused by name: it is either
  this appliance (an endless loop) or the stub resolver of the daemon being
  configured, and either way the LAN loses resolution with no hint why.
- **Names are grammar-checked, not escaped.** A name must be RFC 1035
  (letters, digits, `-`, `.`), lowercased, one trailing dot dropped; `*`, `/`,
  spaces, `#` and control characters are refused because `dnsmasq.conf` has no
  quoting that would make them safe on that line.
- **An alias must point at a name the appliance serves.** dnsmasq silently
  ignores a `cname=` whose target it cannot resolve; here it is a `400` naming
  the target.
- **One answer per question.** The same name twice, the same domain twice, the
  same forwarder twice, or a domain entry and a name record for the same name
  are all refused (`400`).
- **Answering for DHCP clients replaces the list.** With
  `serve_dhcp_clients`, the DHCP card's `dhcp-option=6` list is dropped so
  dnsmasq advertises its own address — an explicit list *and* that behaviour
  would be two answers for the same DHCP option. The switch is a `400` while the
  DNS server is off (it would mean nothing).
- **Bounded.** At most 3 forwarders, 100 local names, 100 answered domains, and
  an interface name no longer than the kernel's 15-character limit.
- **Every subprocess has a deadline** (`CONFIG_WRITE_TIMEOUT`,
  `DNSMASQ_TIMEOUT`, `PROBE_TIMEOUT`) through
  `utils::process::run_timed_async*`, which the tree-wide source guard
  (`utils::process::tests::every_subprocess_of_the_tree_is_bounded`) enforces.

## What a save does to the host

| State after the save | Effect |
|---|---|
| any of DHCP / TFTP / DNS on | writes `/etc/dnsmasq.d/homevault.conf`, `systemctl restart dnsmasq`, verifies it stayed up |
| all three off | removes the drop-in, stops `dnsmasq` |
| DNS off while DHCP/TFTP is on | the DNS section becomes a comment; the daemon keeps running for DHCP |
| host refuses the configuration | the previous document is restored and re-applied, and the API names the failure |

A `dnsmasq` that reports success but is not running afterwards is an error, not
a success — the reconcile path re-reads the unit state.

## Tests

| Suite | What it covers |
|-------|----------------|
| `src/local_dns.rs` (`#[cfg(test)]`, 22) | hostname/domain grammar and injection refusals, answer-address rules, loopback forwarder, the three enable gates, alias targets, one-answer-per-name, bounds, the rendered directives (off emits none; on emits `no-resolv`/`domain-needed`/`bogus-priv`/`local-service` and one line per entry), `resolv.conf` parsing, the config path |
| `src/dhcp_server.rs` (`#[cfg(test)]`, +5) | `render_conf` binds the serving interfaces once each, carries the DNS section, drops `dhcp-option=6` when the appliance advertises itself, and `daemon_wanted` |
| `tests/local_dns_integration.rs` (14) | `401`/`403` matrix, the guard running before the body is parsed, the GET payload, every `400` refusal, the document round-trip through the module's own persistence helpers |
| `tests/phase4_auth_integration.rs` | the two routes join the Phase 4 access-control matrix (member reads, admin-only write, an admin never refused) |
| `tests/e2e/settings/local-dns.spec.ts` (12) | capability-absent, off by default, interface chooser, the three editors, the confirmation, a refused save, the shared-daemon badge, keyboard/labels, no value in URL or `localStorage`, 360/768/1920 |
| `tests/dhcp_server_integration.rs` (21) | unchanged, still green against the recomposed drop-in |

**Never exercised live (deliberately, still owed):** a real `systemctl restart
dnsmasq` on a machine whose networking is under test — that would rewrite the
test host's resolver. The rendered file was syntax-checked against the real
dnsmasq 2.91 (`dnsmasq --test`, both a good and a deliberately broken file: the
good one passes, the broken one is rejected), and every E2E call is intercepted.
A `dig` round trip belongs on a disposable VM.

## Related

- [DHCP / TFTP / PXE server](./dhcp.md) — the other half of the same daemon
- [Network interfaces and diagnostics](./network.md) — the interface list the
  DNS card chooses from
- [mDNS](./mdns.md) — the other way a name reaches the LAN
- [Firewall](./firewall.md) — `dnsmasq` needs UDP/TCP 53
