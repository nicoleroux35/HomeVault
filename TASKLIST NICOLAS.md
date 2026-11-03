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

- [ ] **Dependabot: 1 low-severity advisory on `main`**
      (`github.com/nicoleroux35/HomeVault/security/dependabot/1`).
      Open since before the Phase A work, reported on every push.
      Not triaged: needs a `cargo audit` run to see whether the vulnerable
      crate is reachable, then either a bump or a documented dismissal. **S**

---

## 2. Phase B — six items, two done

- [x] **B2 — ISO/IMG mounting (read-only)** · shipped `58fe84b`
      `src/iso_mount.rs` + `IsoMountPanel.svelte` · guide `iso-mount.md`
      25 unit + 11 integration.
- [x] **B3 — SMS alert channel** · shipped `8e33dd4`
      `src/sms.rs` + Settings form · guide `sms.md`
      19 unit + 10 integration.

- [ ] **B1 — File tags** · **M**
      Migration `075_file_tags.sql` (`path`, `tag`, `owner`),
      `GET/POST/DELETE /api/explorer/tags`, tag filter in the Explorer sidebar
      next to Favorites, tag chips in the ⌘K overlay.
      *Why it is last of the three:* it is the only one needing a migration.
      *Pattern to copy:* Favorites already has the API + sidebar shape
      (`handle_explorer_favorites_*`, `Explorer.svelte:1690`).
      *Watch:* `resource_shares` and the ACL model — a tag must not become a way
      to learn that a file exists outside the caller's roots.
      Verified absent: no `file_tags` in `sqlx/migrations/`.

- [ ] **B4 — SNMP agent** · **M**
      The inverse of `src/snmp.rs`, which is a **client** (`snmpget`/`snmpwalk`
      against remote devices). Generate `/etc/snmp/snmpd.conf` (community, bind
      address, sysLocation), start/stop `snmpd`, expose `GET/PUT /api/snmp/agent`.
      *Why:* `snmpd` is installed by `install.sh` and only *probed* by
      `src/dependencies.rs:819`; nothing configures it. The installer already
      drops a view drop-in, so half the plumbing exists.
      *Rules:* off by default, LAN-bound, never `public` as the default
      community, `503` when `snmpd` is absent (P0.2 contract).
      Verified absent: no `/snmp/agent`, no `snmpd.conf` writer.

- [ ] **B5 — Link aggregation** · **M**
      `InterfaceType::Bond` + `POST /api/network/bonds` (802.3ad /
      active-backup, member NICs), persisted through the existing
      `persist_interface_config` + `apply_persisted_config` rollback path in
      `src/network.rs`.
      *Risk:* this edits host networking. The apply/verify/roll back path at
      `docs/guides/network.md` §"Apply, verify, roll back" is the model, and a
      failed apply must not strand the admin off the box.
      Verified absent: no `Bond` variant in `InterfaceType`.

- [ ] **B6 — VLAN configuration** · **M**
      `InterfaceType::Vlan` already exists for **detection only**
      (`src/network.rs:188`, `:1846`). Extend it into creation: parent
      interface + VID → `ip link add link … type vlan`, same
      persist/apply/rollback path as B5.
      *Do B5 and B6 together* — same file, same risk, same rollback.

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

1. **Dependabot** (§1) — one command to triage, and it is the only thing
   flagged by GitHub.
2. **B5 + B6 together** — one file, one risk model, one test pass.
3. **B1** — visible to the user, and Favorites proves the shape.
4. **B4** — the installer already ships half of it.
5. **C1 → C2 → C3**.
6. **D2 → D1**.

Phases B–D have no blocker between them. Each item ships with its guide,
`API.md` entry, `FeatureDoc`, unit + integration + E2E tests, and `TESTING.md`
counts, per the definition of done in `CLAUDE.md`.
