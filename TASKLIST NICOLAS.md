# TASKLIST NICOLAS

Working checklist of what is **left to implement**. Derived on 2026-11-03 by
grepping the tree, not by trusting a document: every "shipped" claim below was
checked against `src/`, `sqlx/migrations/`, `docs/guides/` or `frontend/src/`.

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

- [ ] **C1 — Local DNS server** · **M**
      Extend the dnsmasq app: local A/CNAME records, upstream forwarders,
      `address=/domain/ip` entries, and a "this appliance is the LAN DNS"
      switch. Reuse `src/dhcp_server.rs`'s config writer and capability gate
      rather than adding `bind9` — a second daemon contradicts the 1 GB target.
      *Do C1 before C2* (C2's host rules will want names to resolve).
      Verified absent: no `address=/domain` generation anywhere.

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

## 6. Document hygiene (done 2026-11-03)

- [x] `ROADMAP-DSM.md` "Gap summary" refreshed — 21 of its 23 rows had claimed
      "none" for code that had shipped (`rsync`, `iSCSI`, `replication`,
      `snapshot locks`, `backup target`, `config backup`, `directory SSO`,
      `Security Advisor`, `fan control`, `UPS server`, `DHCP`, `USB copy`,
      `guided redundancy`, Time Machine, traffic control).
- [x] `dsm_homevault_comparison.md` re-checked: of its 18 "🔵 À IMPLÉMENTER"
      rows, **11 are shipped**, 5 are open (content search, tags, DNS, bond,
      VLAN, reverse proxy — 6 counting SNMP's client-only half), 1 is rejected
      (QuickConnect) and 1 is the PWA's missing push half.
      **The file is left untouched on purpose** — it is the dated input record,
      and §2 of the implementation plan is its reconciliation. It is untracked
      (repo rule: no standalone root `.md` in commits).

---

## 7. Suggested order

1. ~~**B5 + B6 together**~~ — shipped 2026-11-04.
2. ~~**B4** (SNMP agent)~~ — shipped 2026-11-04. Phase B is complete.
3. **C1 → C2 → C3**. ← **next**
4. **D2 → D1**.

Phase B is complete (B1–B6). Phases C and D have no blocker between them. Each item ships with its guide,
`API.md` entry, `FeatureDoc`, unit + integration + E2E tests, and `TESTING.md`
counts, per the definition of done in `CLAUDE.md`.
