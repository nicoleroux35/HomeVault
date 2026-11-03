# DSM parity — implementation plan

Derived from `dsm_homevault_comparison.md` (v1.0, 2026-10-30) and reconciled
against the working tree on 2026-11-02.

| Field | Value |
|---|---|
| Status | Proposal. Nothing here is committed scope until it moves to `In progress` in `ROADMAP-DSM.md` and gets a guide. |
| Source document | `dsm_homevault_comparison.md` — a capability comparison, **not** a work plan. Kept outside the tree (repo rule: no scratch/reference files in commits); §2 is the reconciliation that makes it unnecessary to read |
| Canonical roadmap | `ROADMAP-DSM.md` (this plan feeds it; it does not replace it) |
| Baseline | HomeVault 1.0.0, `HEAD = e586f33` |
| Method | Every claim in the source document was re-checked against `src/`, `frontend/src/`, `sqlx/migrations/`, `docs/guides/`, `tests/` and `src/web/mod.rs` route wiring |

---

## 1. Why this plan is not a transcription of the source document

The source document was written from a static DSM firmware extraction and a
partial reading of HomeVault. Re-checking it against the tree shows that
**13 of its 20 "À IMPLÉMENTER / À VÉRIFIER" items are already shipped**, and
that the real risk is not a missing feature but a **conformance and security
debt on the features shipped last** (Phase 4, commit `e586f33`).

Two consequences for the plan:

1. **Do not build what exists.** Building "DHCP Server" or "USB Copy" again
   would duplicate `src/dhcp_server.rs` and `src/usb_copy.rs`.
2. **Fix the last tranche before adding a new one.** Five Phase 4 handler
   modules expose privileged, root-executing endpoints with **no session
   check at all** (§3.1). That outranks every feature gap in the source
   document.

---

## 2. Reconciliation — source claim vs. verified state

### 2.1 Claims that are already delivered (drop from the plan)

| Source item | Verified evidence | Verdict |
|---|---|---|
| #2 DHCP Server | `src/dhcp_server.rs` (dnsmasq DHCP+TFTP+PXE), `src/handlers/dhcp_server.rs`, 5 routes in `src/web/mod.rs:1518-1522`, `DhcpServerCard.svelte`, `tests/dhcp_server_integration.rs` | **Shipped** (P4.3) |
| #4 USB Copy | `src/usb_copy.rs`, `src/handlers/usb_copy.rs`, 6 routes (`:1511-1516`), `UsbCopy.svelte`, `tests/usb_copy_integration.rs` | **Shipped** (P4.2) |
| #5 Text editor | Inline editor in the Explorer preview modal, `POST /explorer/save` (atomic temp+rename, 5 MB cap) — `docs/guides/explorer.md:52,551` | **Shipped** |
| #6 VPN Client | OpenVPN client connections with tunnel verification (`src/vpn.rs:816` `start_openvpn_connection`, `.ovpn` upload hardening `:640`), WireGuard peer provisioning, `VPN.svelte` client tab | **Shipped** |
| #7 ACME / Let's Encrypt | `src/acme.rs`, `src/acme_worker.rs`, `docs/guides/acme.md` | **Shipped** (DSM-01) |
| #9 Favoris | `GET/POST/DELETE /api/explorer/favorites` (`src/web/mod.rs:1634-1643`), sidebar section in `Explorer.svelte:1690` | **Shipped** (tags are not — see §2.2) |
| #11 Dashboard widgets | `EditWidgets.svelte`, migration `057_widgets_per_user.sql`, `src/handlers/monitor_layout.rs` | **Shipped** |
| #19 Hot spare | `src/disk.rs:1443,3502`, `src/guided_redundancy.rs:458`, `use_spares` i18n key | **Shipped** |
| #21 Quotas par partage | Migration `066_share_quotas.sql`, QW-07, `docs/guides/quotas.md` | **Shipped** |
| #23 SHR-like | `src/guided_redundancy.rs` + `GuidedRedundancyDialog.svelte` | **Shipped** (P4.5) |
| #14 SNMP (partially) | `src/snmp.rs` + `SnmpCard.svelte` — but **client only** (queries remote devices) | **Partial** — see §2.2 |
| #1 Universal Search (partially) | `GET /api/search/files` (QW-12), `src/global_search.rs`, `docs/guides/global-search.md` — **filename only** | **Partial** — see §2.2 |
| #10 Mobile apps (partially) | `frontend/public/manifest.json` + `sw.js` with `push`/`notificationclick` handlers | **Partial** — no backend push |

### 2.2 Claims that are genuinely open

| Source item | Verified state | Real gap |
|---|---|---|
| #1 Content search | `docs/API.md:235` states explicitly "**Name search only** (no content index)" | Full-text index over file contents |
| #3 DNS Server | `dnsmasq` is used by the DHCP app, but there is no DNS zone/forwarder UI and no `bind9`/`named` integration | Local DNS zones + forwarders |
| #8 Reverse proxy UI | Only the **per-app** proxy (`src/app_proxy/`, `docs/guides/apps-center.md`). No user-defined host→upstream reverse proxy | Generic reverse proxy |
| #9 Tags | No `tags` table for files (only `restic`/`backup` tag columns) | File tags + tag search |
| #12 Push notifications | `sw.js` handles `push`, but no subscription endpoint, no VAPID keys, no sender | Web Push backend |
| #13 SMS | `src/notification_engine.rs` dispatches email + webhook only | SMS channel |
| #14 SNMP agent | `snmpd` is only a *dependency check* (`src/dependencies.rs:819`); HomeVault never configures or exposes it | SNMP agent (expose the appliance) |
| #15 Link aggregation | `src/network.rs` has no bonding code; `InterfaceType` has no `Bond` | Bonding UI |
| #16 VLAN | `InterfaceType::Vlan` exists (`src/network.rs:188`) for **detection only**; no creation | VLAN configuration |
| #17 Port forwarding / UPnP | No `upnp`/`miniupnp`/NAT code anywhere | UPnP-IGD client |
| #20 ISO mount | No `losetup`/loop-mount code | ISO mount in Explorer |
| #18 QuickConnect-like | Deliberate non-goal (`ROADMAP-DSM.md` → Non-goals) | **Reject** — see §6 |
| #22 HA cluster, #24 L2TP, #25 Surveillance, #26 Photos, #27 Mail, #28 Chat, #29 k8s, #30 UPnP | Niche / out of scope per `ROADMAP-DSM.md` | **Reject or defer** |

---

## 3. Findings that reorder the priority

### 3.1 P0 — Five Phase 4 handler modules have no authentication

`src/handlers/admin_guard.rs` states the tree's contract: *"This module is the
only place in the tree that may turn an `x-session-token` header into an
identity decision… a handler guards itself with a single match."* There is **no
global auth layer** on `/api/*` — the layers in `src/web/mod.rs:2260-2289` are
`auto_block`, `csrf`, `contract` and `password_change_guard`, none of which
authenticate.

Measured on the five modules added by `e586f33`:

| Module | `require_auth` | `require_admin` | `HeaderMap` |
|---|---|---|---|
| `src/handlers/dhcp_server.rs` | 0 | 0 | 0 |
| `src/handlers/usb_copy.rs` | 0 | 0 | 0 |
| `src/handlers/fan_control.rs` | 0 | 0 | 0 |
| `src/handlers/ups_server.rs` | 0 | 0 | 0 |
| `src/handlers/guided_redundancy.rs` | 0 | 0 | 0 |

Compare `src/handlers/iscsi.rs`, which guards every route with
`require_auth`/`require_admin` (`:452,472,521,574,638`).

**Exposed without a session**, among 22 routes:

- `POST /api/power/ups-server/shutdown` → runs `sudo poweroff`
- `POST /api/network/dhcp-server/start|stop` → `sudo systemctl start dnsmasq`
- `PUT /api/system/fans/config` → writes hwmon PWM
- `POST /api/usb-copy/execute/:id` → copies removable media to a destination
- `POST /api/storage/guided-redundancy/apply/:id`

This is a **privilege-escalation and denial-of-service surface on the LAN**, and
it violates the project rule "least privilege (`require_auth` reads,
`require_admin` mutations)". It must be closed before any new feature ships.

### 3.2 P0 — Phase 4 does not meet the project's own definition of done

`ROADMAP-DSM.md` → "Definition of done" requires code, API, UI, **documentation,
unit tests, integration tests and E2E tests** in the same change. Phase 4
delivered code + unit + integration only:

| Requirement | Phase 4 state |
|---|---|
| i18n | **0** `$t(` calls across `DhcpServerCard`, `UpsServerCard`, `FanControlCard`, `UsbCopy`, `GuidedRedundancyDialog` — every string is a hardcoded English literal (e.g. `DhcpServerCard.svelte:132,144,161`). Violates `docs/I18N.md` and the "accessible names from an i18n key" rule. |
| Guides | No `docs/guides/dhcp.md`, `usb-copy.md`, `fan-control.md`, `ups-server.md`, `guided-redundancy.md`. `docs/PHASE4_IMPLEMENTATION.md:60` itself says *"`docs/guides/ups-server.md` (to be written)"*. |
| `docs/INDEX.md` | None of the five areas listed |
| `docs/API.md` | **0** of the 22 endpoints documented |
| `SCREENS.md` | None of the five screens listed |
| RAG `FeatureDoc` | None — and `src/rag_indexers.rs::tests` asserts the indexed guide set equals `docs/guides/` exactly, so adding guides without `FeatureDoc`s breaks the test |
| E2E | No `tests/e2e/{usb-copy,dhcp,fan,ups,guided-redundancy}/` directory exists |
| `TESTING.md` | Counts not updated for the 195 Phase 4 tests |

### 3.3 Correctness defects found while auditing

| # | Defect | Evidence |
|---|---|---|
| D1 | `docker stop -t 30 "$(docker ps -q)"` is passed as **argv**, not through a shell, so the literal string `$(docker ps -q)` is treated as a container name — the stage stops nothing and still reports success | `src/ups_server.rs:452-455` |
| D2 | `shutdown_vms` and `flush_restic` stages are `Ok(())` stubs that report success without doing anything | `src/ups_server.rs:461-471` |
| D3 | `guided-redundancy/apply` does not create anything; it returns "create the layout manually via Storage screen" | `src/handlers/guided_redundancy.rs:51-59` |
| D4 | `usb_copy` and `dhcp_server` shell out to `sudo`/`udisksctl`/`systemctl`; the deadline discipline required by `CLAUDE.md` ("every privileged call has a deadline") was not verified for these modules | `src/usb_copy.rs:264-369`, `src/dhcp_server.rs:259-444` |

---

## 4. Phased plan

Effort scale from `ROADMAP-DSM.md`: **S** ≤ 2 d, **M** 3–5 d, **L** 2–3 wk,
**XL** ≥ 4 wk.

### Phase A — Close the Phase 4 debt (P0, do first)

| ID | Work item | Effort |
|---|---|---|
| **A1** | **Authenticate the 22 Phase 4 routes.** Add `HeaderMap` to every handler; `require_auth` on reads, `require_admin` on every mutation. Add an integration test per module asserting `401` without a token and `403` for a non-admin, mirroring `tests/iscsi_integration.rs`. | S |
| **A2** | **Fix D1/D2/D3.** Replace the `$(docker ps -q)` argv with a two-step `docker ps -q` → `docker stop` (no shell); make `shutdown_vms`/`flush_restic` call the real services or return an explicit `skipped` result instead of `Ok`; make `guided-redundancy/apply` either create the array through `disk.rs` or refuse with `501` and say so in the UI. | M |
| **A3** | **i18n the five components.** Extract every literal into `src/i18n/{en,fr,de,es}.toml` under `[dhcp]`, `[usb_copy]`, `[fan_control]`, `[ups_server]`, `[guided_redundancy]`; icon-only buttons get `aria-label` from a key. | M |
| **A4** | **Documentation.** Write `docs/guides/{dhcp,usb-copy,fan-control,ups-server,guided-redundancy}.md`; add the 22 endpoints to `docs/API.md`; add the five screens to `SCREENS.md`; add the five rows to `docs/INDEX.md`; add five `FeatureDoc`s to `src/rag_indexers.rs` and the "Document Sources" row in `docs/guides/rag.md` (the `FEATURE_GUIDES` test enforces the mapping). | M |
| **A5** | **E2E + counts.** `tests/e2e/{dhcp,usb-copy,fan-control,ups-server,guided-redundancy}/*.spec.ts` covering the capability-absent path (the honest no-op), the admin path and the non-admin refusal; update `TESTING.md` counts. | M |
| **A6** | **Deadline audit (D4).** Verify every `Command` in the five modules is wrapped in `tokio::time::timeout` with a named constant, as `src/snmp.rs:84` does. | S |

**Exit criteria:** `cargo test` green, `npm run check` green, the five E2E
suites green, `docs/INDEX.md`/`API.md`/`SCREENS.md`/`TESTING.md` updated, and a
`curl` without a session token returns `401` on all 22 routes.

### Phase B — Genuinely missing quick wins (S each)

| ID | Work item | Effort | Notes |
|---|---|---|---|
| **B1** | **File tags** (source #9) | M | Migration `075_file_tags.sql` (`path`, `tag`, `owner`), `GET/POST/DELETE /api/explorer/tags`, tag filter in the Explorer sidebar next to Favorites, tag chips in the ⌘K overlay. Favorites already prove the pattern. |
| **B2** | **ISO mount** (source #20) — **shipped 2026-11-02** ✅ | S | Delivered as `src/iso_mount.rs` + `src/handlers/iso_mount.rs` + `IsoMountPanel.svelte`; `docs/guides/iso-mount.md`. Route is `/api/explorer/iso`, not `mount-iso`. Mounts are `ro,nosuid,nodev`; the mount point is **derived** from a deterministic slug under `{DATA_DIRECTORY}/iso`, never taken from the request; the image must sit inside the caller's Explorer roots; a `..` is refused before `canonicalize`; `/proc/mounts` is the authority, not a JSON index; a failed mount detaches its loop device. Tests: 20 unit (module), 5 unit (handlers), `iso_mount_integration` **11/11**. Deviation: no automated successful-mount round trip (no passwordless sudo here, and a real mount would leave a device attached) — argv and containment are asserted directly, as `iscsi.md` already records for its hardware suite. |
| **B3** | **SMS notification channel** (source #13) | S | Add a `sms` variant to `src/notification_engine.rs` beside email/webhook; provider-agnostic HTTP POST with a URL template, so no vendor SDK is added. Secrets sealed like `webhook_url`. |
| **B4** | **SNMP agent** (source #14) | M | The inverse of `src/snmp.rs`: generate `/etc/snmp/snmpd.conf` (community, bind address, sysLocation), start/stop `snmpd`, expose `GET/PUT /api/snmp/agent`. Off by default, LAN-bound, never `public` by default. |
| **B5** | **Link aggregation** (source #15) | M | `InterfaceType::Bond` + `POST /api/network/bonds` (mode 802.3ad/active-backup, member NICs), persisted through the existing `persist_interface_config` + `apply_persisted_config` rollback path in `src/network.rs`. |
| **B6** | **VLAN configuration** (source #16) | M | Extend the existing `InterfaceType::Vlan` detection into creation: parent interface + VID → `ip link add link … type vlan`, same persist/apply/rollback path. |

### Phase C — Network depth (M)

| ID | Work item | Effort | Notes |
|---|---|---|---|
| **C1** | **Local DNS server** (source #3) | M | Extend the dnsmasq app: local A/CNAME records, upstream forwarders, `address=/domain/ip` entries, and a "this appliance is the LAN DNS" switch. Reuse `src/dhcp_server.rs`'s config writer and capability gate rather than adding `bind9`. |
| **C2** | **Generic reverse proxy** (source #8) | L | Host → upstream rules (`host`, `path`, `upstream`, `tls`), reusing `src/app_proxy/`'s origin resolution and the auth gate. Must not weaken the per-app proxy's "never trust a forged header" rule (`docs/guides/apps-center.md:90`). |
| **C3** | **UPnP-IGD port forwarding** (source #17) | M | `miniupnpc` client: list the router's mappings, add/remove one, show the external IP. Read-only discovery first; every mutation confirmed and reversible. |

### Phase D — File intelligence (L)

| ID | Work item | Effort | Notes |
|---|---|---|---|
| **D1** | **Content index** (source #1) | L | The only item the source document ranks correctly as high effort. Design constraints before code: opt-in per root, explicit exclusion list, ACL-filtered results (reuse `explorer_roots_for_session`), bounded index size, retention, and a "delete my index" action. Prefer an embedded index (SQLite FTS5, already in the tree) over adding Meilisearch/Elasticsearch — a new daemon contradicts the 1 GB RAM target. |
| **D2** | **Web Push backend** (source #12) | L | `sw.js` already handles `push`; add VAPID key generation, `POST /api/notifications/subscribe`, a sender in `src/notification_engine.rs`, and per-device revocation. Gate behind explicit consent (RGPD) and document the delivery semantics. |

### Phase E — Deferred / rejected

| Source item | Decision | Reason |
|---|---|---|
| #18 QuickConnect-like relay | **Reject** | Vendor relay = central infrastructure, account dependency, third-party traffic. `ROADMAP-DSM.md` already rejects it; ACME + DDNS + reachability + VPN cover the job. |
| #10 Native mobile apps | **Defer** | The installable PWA exists (`manifest.json`); finish D2 (Web Push) first. |
| #22 HA cluster, #24 L2TP, #25 Surveillance, #26 Photos, #27 Mail, #28 Chat, #29 k8s | **Reject** | Niche or out of scope; the Docker catalog already covers Photos/Chat/Surveillance. |
| #30 UPnP (as a *server*) | **Reject** | C3 covers the useful half (client). |

---

## 5. Sequencing

```
Phase A (P0)  ──►  Phase B (S, parallelisable)  ──►  Phase C (M)  ──►  Phase D (L)
   A1 first          B1, B2, B3 independent          C1 before C2       D1 needs a
   A2..A6 after      B4, B5, B6 touch network        C2 needs C1        design note
```

- **A1 is a hotfix**, not a feature: it can ship alone, in one commit, with its
  integration tests.
- **B5/B6 must land after A6** so the network module's deadline discipline is
  already established.
- **D1 must not start** before a written design note covering privacy, ACL,
  exclusion and retention — the source document's "ElasticSearch/Meilisearch"
  suggestion is rejected on footprint grounds.

---

## 6. Guardrails for every item

Inherited from `CLAUDE.md` and `ROADMAP-DSM.md`; restated because the source
document ignores them:

1. **Least privilege.** `require_auth` on reads, `require_admin` on mutations.
   No new route ships without its `401`/`403` integration test.
2. **Capability detection.** Hardware- or tool-dependent features (B4, B5, B6,
   C1, C3) must detect the capability and refuse with a named reason, never
   fail silently. `src/dhcp_server.rs::is_available` is the reference pattern.
3. **Deadlines.** Every privileged subprocess gets a `tokio::time::timeout`
   with a named constant.
4. **No shell interpolation.** `Command::new(...).args([...])` only — the D1
   defect is exactly what happens when a shell expression is passed as argv.
5. **i18n + a11y.** Every string from a TOML key; icon-only buttons carry
   `aria-label`; 44 px hit areas; focus visible; one `<h1>` per screen.
6. **Simplicity.** Each new screen answers "what does the user do next" in its
   empty state; advanced options behind a details panel.
7. **Docs in the same change.** Guide + `docs/INDEX.md` + `docs/API.md` +
   `SCREENS.md` + `TESTING.md` + `FeatureDoc` + `docs/guides/rag.md` row.
8. **Tests in the same change.** Unit (`#[cfg(test)]`), integration
   (`tests/*_integration.rs`), E2E (`tests/e2e/<screen>/`).

---

## 7. Definition of done

An item moves to `implemented` only when code, migration/config, API, UI,
documentation, unit tests, integration tests and E2E tests are present. It moves
to `verified` only after the acceptance tests run on the relevant capability, or
a documented hardware-independent fallback proves the unsupported path. Update
`ROADMAP-DSM.md`, `TESTING.md` and the guide in the same change.

---

## 8. Housekeeping — done 2026-11-02

- **`docs/PHASE4_IMPLEMENTATION.md`** claimed "E2E pending" and "guide to be
  written". Both became false when Phase A shipped; the file was superseded by
  `ROADMAP-DSM.md` → Phase 5 and has been **deleted**.
- **`docs/DENSITY_QUICK_REFERENCE.md`, `docs/DENSITY_SUMMARY.txt`,
  `docs/DENSITY_SYSTEM_PROPOSAL.md`** were French-language scratch notes for an
  unimplemented density feature, at the docs root, unreferenced by anything.
  **Deleted.** A density proposal that is ever picked up should start as a
  proper English guide under `docs/guides/`, not as three competing text files.
- **`dsm_homevault_comparison.md`** is the source document this plan derives
  from. It stays at the repo root, unmodified, as the input record: §2 above is
  the reconciliation, and `ROADMAP-DSM.md` remains the canonical roadmap. It is
  deliberately **not** committed — the project rule forbids scratch/reference
  files in the tree, and a document whose claims are 13-for-20 stale would
  mislead the next reader far more than its absence would.

## 10. P0.1 — quick-win E2E verification — closed 2026-11-02

The roadmap's Phase 0 carried two items that predate this plan. **P0.1 is now
closed**; P0.2 is the only one left.

The first pass (2026-10-31) recorded **39 passed / 32 failed / 1 skipped** and
classified part of the failures as an *environment limitation*, because the
scratch host had no passwordless sudo. The rerun on a rebuilt binary, isolated
`DATA_DIRECTORY` and one worker:

| Browser | Result |
|---|---|
| Firefox | **70 passed, 2 skipped, 0 failed** |
| Chromium | **70 passed, 2 skipped, 0 failed** |

The 32 failures are gone; both skips are state guards (`test.skip(condition,
reason)`), not defects.

**The sudo limitation was measured, not assumed.** It is real on this host
(`sudo -n true` demands a password, `POST /api/groups` fails with the command's
own stderr) but **none of the six suites touches a privileged path** — no
`/api/groups`, `home_enabled`, `delete_home`, `getent`, `uid` or `/etc/passwd`
reference exists in them. `ensure_linux_user` is reached only when a group is
added, never when a user is created, so the limitation is out of scope here.

Detail in `TESTING.md` → "P0.1 — Quick-win E2E verification pass closed";
`ROADMAP-DSM.md` carries the status.

**Remaining in Phase 0: P0.2** — cross-cutting capability/API contracts
(uniform unsupported-capability responses, admin-mutation/secrets/audit/
`requires_restart` conventions) before any Phase B item starts.

## 12. B2 — read-only ISO mounting — shipped 2026-11-02

First item of Phase B. Design decisions worth keeping:

- **The mount point is derived, never requested.** `<DATA_DIRECTORY>/iso/<slug>`
  with `slug = <readable-name>-<16 hex of the full path>`. A client cannot name
  a directory; the hash makes `ubuntu.iso` in two folders mount side by side
  and re-mounting reuse the same directory instead of leaking one per attempt.
- **The image must already be readable by the caller.** The roots come from
  `explorer::explorer_roots_for_session`, the exact function the sidebar uses —
  so the feature inherits the existing confinement rather than re-deriving it.
- **`ro,nosuid,nodev`, always.** A writable mount would let a browse mutate the
  source image; `nosuid`/`nodev` stop a crafted image introducing a setuid
  binary or a device node into the appliance's namespace.
- **`/proc/mounts` is the authority.** `mounts.json` only carries the label and
  loop device of a mount the filesystem already confirms; a directory with no
  mount on it is never reported as mounted.
- **A failed mount detaches its loop device**, so an image that is not a
  filesystem cannot leak a device on the host.
- **Statuses follow P0.2**: `400` when the refusal is about the request,
  `503` when `losetup`/`mount` is absent.

`cargo test --lib` went from 2,379 to **2,404**; `iso_mount_integration` is
**11/11**. The five pre-existing lib failures are untouched and unrelated.

## 11. P0.2 — capability and API contracts — closed 2026-11-02

The gap was **status drift**, not missing thinking: a missing external tool was
answered `400` — the status reserved for a request the server could not
understand — at 32 call sites (`docker` 26, `restic` 3, `firewall`, `antivirus`,
`backup_target`). A client could not tell "fix your input" from "install a
package".

| Contract | Status | Helper |
|---|---|---|
| Missing capability (tool/module/hardware absent) | **`503`** | `ResponseHelper::json_unavailable(missing)` |
| Feature this build cannot perform | **`501`** | `ResponseHelper::json_not_implemented(message)` |
| Malformed request | `400` | unchanged |
| Missing/unknown session · non-admin | `401` · `403` | `admin_guard` |

All 32 sites converted; `grep 'json_error("… not available\|not installed")'`
returns nothing. The contract — capability discovery field names, admin
mutations, audit, `requires_restart`, secrets — is written in `docs/API.md` →
Conventions, and `cargo test --lib utils::response` (4/4) keeps the three
statuses distinct.

**Phase 0 is now complete.** Every Phase B item starts from a documented
endpoint shape, a named unsupported-capability status and an explicit privilege
boundary.

## 9. Two follow-ups raised in review — closed 2026-11-02

Both were found while verifying Phase A end to end.

### Settings had no launcher tile

The screen that holds every category of server configuration was reachable from
the dock and from ⌘K, but was **the one app missing from the launcher grid**.
`Desktop.svelte` now composes `settingsTile` into `allApps`, so it appears in
"All apps" (chip `System`), is searchable, and opens through the shared
`openApp` → `focusExistingWindow` path.

### ⌘K leaked host paths ahead of navigation

Typing `Settings` in the ⌘K overlay filled all eight visible rows with files
from the host's own filesystem — `settings.png`, `Settings.java`,
`settings.gradle.kts` under `/home/<user>/…` — before "Open Settings" was ever
reachable with the arrow keys.

`mergeFileHits` (`frontend/src/lib/stores/search.ts`) appended file hits after
the local index **as it stood when the async answer landed**, which is not the
same list as the current query's. It now sorts every navigation row (`app`,
`setting`, `action`) before any `file` row, caps the merged list at
`MAX_RESULTS` (8), and clamps `selectedIndex` so a shorter list can never leave
the selection past the end. Documented in `docs/guides/global-search.md`
(ordering rule + invariant) and `docs/guides/desktop.md` (launcher tile).
