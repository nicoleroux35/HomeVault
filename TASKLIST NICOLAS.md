# TASKLIST NICOLAS

Working checklist of what is **left to implement**. Derived on 2026-11-03 by
grepping the tree, not by trusting a document: every "shipped" claim below was
checked against `src/`, `sqlx/migrations/`, `docs/guides/` or `frontend/src/`.
Updated **2026-11-05**: Phase B is complete (B1 and B4–B6 shipped on
2026-11-04; `ffdd7f1`, `3df8a7d`, `82b7c1e`), and **C1 shipped on 2026-11-05**.
The next item is **C2**.

Sources, in authority order:
1. `CLAUDE.md` — the rules.
2. `ROADMAP-DSM.md` — the canonical roadmap.
3. `docs/analysis/dsm-parity-implementation-plan.md` — how each item was
   derived, and the phases.
4. `dsm_homevault_comparison.md` — the raw input; **17 of its 18 "À
   IMPLÉMENTER" rows are stale** (see §5).

Legend: **S** ≤ 2 days · **M** 3–5 days · **L** 2–3 weeks · **XL** ≥ 4 weeks.

---

## 1. One open debt item, unrelated to any feature

- [x] **Dependabot: webpki root certificate advisory**
      `cargo audit --json` identified `RUSTSEC-2026-0383` in the resolved
      `webpki-root-certs 1.0.9`. The dependency is part of rustls' root store;
      `cargo update -p webpki-root-certs` moved the lockfile to **1.0.11**,
      which removes the distrusted emSign root. No advisory dismissal was used.
      Tests were not run by request; verify in CI before release.

---

## 2. Phase B — six items, all shipped (2026-11-04)

- [x] **B2 — ISO/IMG mounting (read-only)** · shipped `58fe84b`
      `src/iso_mount.rs` + `IsoMountPanel.svelte` · guide `iso-mount.md`
      25 unit + 11 integration.
- [x] **B3 — SMS alert channel** · shipped `8e33dd4`
      `src/sms.rs` + Settings form · guide `sms.md`
      19 unit + 10 integration.

- [x] **B1 — File tags** · shipped 2026-11-03 (verified)
      Migration `075_file_tags.sql`, owner-scoped `GET/POST/DELETE
      /api/explorer/tags`, Explorer sidebar and a tag filter/action in ⌘K.
      Paths are validated against the caller's current Explorer roots, tags
      never grant access, each path is limited to 20 tags, and account deletion
      purges the rows. See `docs/guides/file-tags.md`.
      **Verified:** `file_tag_repo.rs` 3/3, `explorer_integration.rs` 99/99,
      `global_search_integration.rs` 14/14, E2E `explorer 15.5` green on
      Chromium **and** Firefox, `npm run check` 0/0, build green.
      **Two defects fixed during verification:** a tag on a deleted path could
      never be removed (the sidebar × died with the file), and the file-coverage
      note in ⌘K was hidden whenever the answer had zero hits — which made
      `desktop 6b.3` red on `main`, now green.

- [x] **B4 — SNMP agent** · shipped 2026-11-04
      `src/snmp_agent.rs` + `GET|PUT /api/snmp/agent` +
      `SnmpAgentCard.svelte`. Writes its **own** drop-in
      (`/etc/snmp/snmpd.conf.d/99-homevault-agent.conf`), never the distro's
      file, and drives the `snmpd` unit.
      *Rules honoured:* off by default; scope is loopback or one private LAN
      address (`0.0.0.0` and public addresses refused); community generated
      (32 hex) and `public`/`private` refused by name; read-only view only (no
      routing table, no MAC column); `503` naming `snmpd` when absent; the
      community is never returned by the API.
      Verified: `snmp_agent.rs` 11/11, `snmp_integration.rs` 10/10 (+2),
      E2E `settings/snmp-agent` 5/5 on Chromium **and** Firefox, sudoers guard,
      check 0/0. No live daemon (every E2E call is intercepted).
      See `docs/guides/snmp-agent.md`.

- [x] **B5 — Link aggregation** · shipped 2026-11-04
      `InterfaceType::Bond` + `GET|POST /api/network/bonds` and
      `DELETE /api/network/bonds/{name}` — `802.3ad` / `active-backup`,
      physical-NIC members only, extended in place when the bond exists.
      Persisted through `persist_virtual_device` + `netplan_apply_and_verify`;
      the created device is then configured through the ordinary per-interface
      card (apply / verify / roll back). See `docs/guides/network.md`
      §"Link aggregation and VLAN devices".
      *Risk handled:* a change that would re-role the interface carrying the
      caller's own address is refused **before any `ip` runs**
      (`refuse_if_carries_session_address`, peer from the TCP socket) — the one
      case no verify/rollback step can report.
      Verified: `network.rs` 71/71 (+9), `network_integration.rs` 28/28 (+4),
      E2E `network` §14 green on Chromium, check 0/0. No live apply (it would
      rewrite the test host's networking).

- [x] **B6 — VLAN configuration** · shipped 2026-11-04 (with B5)
      `InterfaceType::Vlan` extended from detection to creation:
      `GET|POST /api/network/vlans` + `DELETE /api/network/vlans/{name}`,
      parent + `VID 1..=4094` → `<parent>.<vid>` via
      `ip link add link … type vlan`, same persistence and same session-safety
      refusal as B5.
      Verified with B5 above.

---

## 3. Phase C — network depth, three items

- [x] **C1 — Local DNS server** · **M** · shipped 2026-11-05
      `src/local_dns.rs` + `GET|PUT /api/network/dns-server` +
      `LocalDnsCard.svelte`. Local A/alias records, `address=/domain/address`
      entries, up to 3 upstream forwarders, and the "tell the LAN's DHCP clients
      to resolve here" switch. **No new daemon**: it rides the `dnsmasq` the DHCP
      card already drives — `dhcp_server::render_conf` composes the *one*
      drop-in from both halves and `reconcile_daemon` is the single gate that
      starts, restarts or stops it.
      *Rules honoured:* off by default and enabling confirmed; `no-resolv` +
      `domain-needed` + `bogus-priv` + `local-service` so no LAN name leaks to
      the ISP's resolver and the appliance is never a public resolver; a loopback
      forwarder refused by name; RFC 1035 names validated (never escaped) so a
      newline or a `/` cannot inject a directive; an alias whose target the
      appliance does not serve is a `400` instead of dnsmasq silently ignoring
      it; one answer per name; the DHCP card's `stop` is a `409` while this is
      on.
      Verified: `local_dns.rs` 22/22, `dhcp_server.rs` +5 (15/15),
      `local_dns_integration.rs` 14/14, `phase4_auth_integration.rs` 6/6,
      `dhcp_server_integration.rs` 21/21, E2E `settings/local-dns` 12/12 on
      Chromium **and** Firefox, `npm run check` 0/0, build green. The rendered
      file was `dnsmasq --test`-ed against the real 2.91. See
      `docs/guides/local-dns.md`.

- [ ] **C2 — Generic reverse proxy** · **L**
      Host → upstream rules (`host`, `path`, `upstream`, `tls`), reusing
      `src/app_proxy/`'s origin resolution and the auth gate.
      **Must not weaken** the per-app proxy's "never trust a forged header"
      rule (`docs/guides/apps-center.md:90`) — that rule is why the per-app
      proxy is safe, and a generic proxy is the obvious way to lose it.
      Verified absent: only the per-app proxy exists.

- [ ] **C3 — UPnP-IGD port forwarding** · **M**
      `miniupnpc` client: list the router's mappings, add/remove one, show the
      external IP. Read-only discovery first; every mutation confirmed and
      reversible. `miniupnpc` would be a new optional dependency.
      Verified absent: nothing.

---

## 4. Phase D — two large items

- [ ] **D2 — Web Push backend** · **L**
      `frontend/public/sw.js:107` already has a `push` handler, but there is
      **no VAPID key, no subscription endpoint, no sender** — the handler can
      never fire. Add key generation, `POST /api/notifications/subscribe`, a
      sender in `src/notification_engine.rs`, per-device revocation.
      *Order:* do this **before** D1 — the service worker is already half
      written, whereas D1 needs a design note first.
      *GDPR:* explicit consent, documented delivery semantics.

- [ ] **D1 — Content index (full-text over file contents)** · **L**
      `docs/API.md:235` states it explicitly: "**Name search only** (no content
      index)".
      *Design note required before the first line of code:* opt-in per root,
      explicit exclusions, ACL-filtered results (reuse
      `explorer_roots_for_session`), bounded index size, retention, and a
      "delete my index" action.
      *Implementation:* SQLite **FTS5** (already in the tree), not
      Meilisearch/Elasticsearch — a new daemon contradicts the 1 GB RAM target.
      Verified absent: no `fts5` in `sqlx/`.

---

## 5. Rejected — do not implement, and why

Kept here so the question does not come back every six months.

| Item | Why not |
|---|---|
| QuickConnect-style relay | Central infrastructure, account dependency, third-party traffic. ACME + DDNS + reachability + VPN already cover the job. |
| Synology C2 / Hybrid Share / Active Insight | Proprietary cloud, vendor account, telemetry — contrary to the product. |
| Native mobile apps | The installable PWA exists; finish D2 first. |
| HA cluster (Pacemaker/DRBD) | Needs fencing, shared identity and split-brain handling. A home appliance is better served by tested restore + replication. |
| L2TP/IPSec, Mail server, Chat, Surveillance | Niche or out of scope; the Docker catalog already covers Chat/Surveillance/Photos. |
| SHR metadata | Proprietary. `guided_redundancy` gives the same advice with standard layouts. |
| AFP | Deprecated by Apple; SMB covers it. |
| Fibre Channel | Datacentre hardware. |
| Kubernetes / k3s | Out of scope for a NAS. |

---

## 6. Document hygiene (2026-11-03, refreshed 2026-11-05)

- [x] `ROADMAP-DSM.md` "Gap summary" refreshed — 21 of its 23 rows had claimed
      "none" for code that had shipped (`rsync`, `iSCSI`, `replication`,
      `snapshot locks`, `backup target`, `config backup`, `directory SSO`,
      `Security Advisor`, `fan control`, `UPS server`, `DHCP`, `USB copy`,
      `guided redundancy`, Time Machine, traffic control).
- [x] `dsm_homevault_comparison.md` re-checked: of its 18 "🔵 À IMPLÉMENTER"
      rows, **16 are now shipped** (the local DNS server joined file tags, bond,
      VLAN and the SNMP agent on 2026-11-05), **2 are open** (content search
      ← D1, reverse proxy ← C2), 1 is rejected (QuickConnect) and 1 is the PWA's
      missing push half ← D2.
      **The file is left untouched on purpose** — it is the dated input record,
      and §2 of the implementation plan is its reconciliation. It is untracked
      (repo rule: no standalone root `.md` in commits).

---

## 7. Suggested order

1. ~~**B5 + B6 together**~~ — shipped 2026-11-04.
2. ~~**B4** (SNMP agent)~~ — shipped 2026-11-04. Phase B is complete.
3. **C2 → C3**. ← **next** (C1 shipped 2026-11-05; C2 can now rely on
   names resolving locally)
4. **D2 → D1**.

Phase B is complete (B1–B6). Phases C and D have no blocker between them. Each item ships with its guide,
`API.md` entry, `FeatureDoc`, unit + integration + E2E tests, and `TESTING.md`
counts, per the definition of done in `CLAUDE.md`.

---

## 8. Session state — 2026-11-05

What the Phase B session and this one shipped, and what they left behind, so the
next one starts from facts rather than from a document.

**Shipped**

| Commit | Item |
|---|---|
| `ffdd7f1` | B1 — private per-user file tags (+ two defects fixed: a tag on a deleted path could not be removed; the ⌘K coverage note was hidden at zero hits, which was making `desktop 6b.3` red on `main`) |
| `3df8a7d` | B5 + B6 — link aggregation and VLAN devices |
| `82b7c1e` | B4 — the local SNMP agent |
| C1 (2026-11-05) | the local DNS server, on the `dnsmasq` the DHCP card already drove — `src/local_dns.rs`, `GET|PUT /api/network/dns-server`, `LocalDnsCard.svelte`, `docs/guides/local-dns.md` |

**Verified on this session's machine**

- `cargo test --lib` — **2,474 passed, 5 failed** (the same five unrelated
  failures `TESTING.md` has tracked since Phase A: `checksum::…evicts…`,
  `settings`/`i18n` density ×2, `sse::progress` ×2 — all in files no session
  touched).
- `tests/local_dns_integration.rs` 14/14, `tests/dhcp_server_integration.rs`
  21/21, `tests/phase4_auth_integration.rs` 6/6,
  `tests/explorer_integration.rs` 99/99,
  `tests/global_search_integration.rs` 14/14,
  `tests/network_integration.rs` 28/28, `tests/snmp_integration.rs` 10/10.
- E2E, live server + scratch `DATA_DIRECTORY` (`PORT=8311`): `settings/local-dns`
  **12/12 on Chromium and 12/12 on Firefox**; `settings/dhcp` and
  `settings/snmp-agent` 13/13 on Chromium (13/13 on Firefox for the pair run
  before the C1 commit).
- `npm run check` 0 errors / 0 warnings every time; `npm run build` green;
  `cargo clippy` clean on the added files, `rustfmt` clean.
- The rendered `dnsmasq` drop-in was syntax-checked with the real `dnsmasq 2.91`
  (`--test` accepts a rendered-shape file and rejects a deliberately broken
  one); no daemon was restarted.

**Known-failing, pre-existing — verified by rebuilding the pre-change tree and
re-running the same tests**

- `network.spec.ts` 2.2, 4.3, 8.2, 10.1, 10.5, 13.1, 13.2 — they need a
  configurable interface and expect an unauthenticated `400` where the build
  answers `401`. Untouched by the network commits.
- The five lib failures above.
- On a developer box whose `/tmp` holds thousands of entries, the Explorer
  suite's `navigateTo`/`openWork` helpers time out; that is the harness, not the
  product (CI's `/tmp` is clean).

**Never exercised live (deliberately), still owed**

- A real bond / VLAN apply and a real `snmpd` start: creating either rewrites
  the networking of whatever machine runs the suite. The code up to the first
  `ip link add` / file write is covered; the mutation itself needs a disposable
  VM with at least two spare NICs.
- `snmpwalk` against the appliance's own agent, same reason.
- A real `systemctl restart dnsmasq` and a `dig nas.lan @<appliance>` round
  trip (C1): the same reason — restarting the resolver on the test host breaks
  the test host. The drop-in was syntax-checked with `dnsmasq --test` instead.
- The DHCP+DNS reconcile under load (two admins saving both cards at once): the
  single gate in `reconcile_daemon` is covered by unit tests, but a race was
  never produced on a real host.

**Next session starts at C2** (generic reverse proxy), which can now rely on
names resolving locally — see §7.
