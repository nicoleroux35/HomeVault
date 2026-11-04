# TASKLIST NICOLAS

Working checklist of what is **left to implement**. Derived on 2026-11-03 by
grepping the tree, not by trusting a document: every "shipped" claim below was
checked against `src/`, `sqlx/migrations/`, `docs/guides/` or `frontend/src/`.
Updated **2026-11-06**: Phase B is complete (B1 and B4–B6 shipped on
2026-11-04; `ffdd7f1`, `3df8a7d`, `82b7c1e`), **C1 shipped on 2026-11-05**, and
**C2 + C3 shipped on 2026-11-06** — Phase C is complete — **D2 shipped on
2026-11-06**, and **D1 shipped on 2026-11-06**. **Every item on this list is
now shipped.**

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

- [x] **C2 — Generic reverse proxy** · **L** · shipped 2026-11-06
      `src/reverse_proxy/` (`mod.rs`, `rules.rs`, `proxy.rs`, `ticket.rs`),
      migration `076_reverse_proxy.sql`, `GET|POST|PUT|DELETE
      /api/proxy/rules`, `GET /proxy-auth` and `ReverseProxyCard.svelte`.
      **One** host-routed listener (default 80,
      `HOMEVAULT_REVERSE_PROXY_PORT`), rules selected by exact host + longest
      segment path prefix, full path forwarded unchanged.
      *Rules honoured:* every client-supplied `Forwarded`/`X-Forwarded-*`/
      `X-Real-IP` header is discarded and replaced from the peer socket; both
      HomeVault cookies are stripped before the upstream; the upstream must
      resolve to a private/loopback/link-local/ULA address at save **and** on
      every connection; a rule cannot point at the listener's own port or the
      dashboard; the auth gate is a cross-host **one-time ticket** exchange
      (120 s, single use, rule-bound) because `hv_app_session` is host-scoped,
      so `hv_session` never leaves the dashboard origin.
      Verified: `reverse_proxy` unit 31/31, `reverse_proxy_integration.rs`
      21/21 (including a real listener against a loopback upstream),
      E2E `settings/reverse-proxy` 11/11 on Chromium **and** Firefox,
      `cargo test --lib` 2,505 passed / the same 5 pre-existing failures,
      `npm run check` 0/0, build green. Live probe of the full
      `/proxy-auth` → callback → `hv_app_session` round-trip on a scratch
      instance; the ticket replayed as `401`.
      See `docs/guides/reverse-proxy.md` and
      `docs/analysis/c2-reverse-proxy-design.md`.

- [x] **C3 — UPnP-IGD port forwarding** · **M** · shipped 2026-11-06
      `src/upnp.rs` + `GET /api/network/upnp` + `POST|DELETE
      /api/network/upnp/mappings` + `UpnpCard.svelte`. **No new daemon**: it
      drives the optional `upnpc` client from **miniupnpc** (added to
      `installer/install.sh` and the dependency inventory; absent, the card
      names the package and mutations are `503`).
      *Rules honoured:* read-only discovery first (`upnpc -l`) and it changes
      nothing; the external client must be a private/link-local/ULA literal
      (loopback, public and host names refused); ports `1..=65535`; lease
      `0` or `60..=604800` with one hour as the default and permanent never
      preselected; the description is ASCII, bounded and prefixed
      `HomeVault`; every mutation is admin-only **and** requires
      `confirmed: true`; no shell (validated argv), no router credential
      stored; every added mapping is removable from the same list, including
      one added elsewhere.
      Verified: `upnp.rs` 16/16, `handlers/upnp.rs` 1/1,
      `upnp_integration.rs` 9/9, E2E `settings/upnp` 9/9 on Chromium **and**
      Firefox (every call intercepted), `cargo test --lib` 2,522 passed /
      the same 5 pre-existing failures, `npm run check` 0/0, build green.
      No live router (see below). See `docs/guides/upnp.md`.

---

## 4. Phase D — two large items

- [x] **D2 — Web Push backend** · **L** · shipped 2026-11-06
      `src/web_push/` (`mod.rs`, `crypto.rs`, `repo.rs`), migration `077`, `GET
      /api/notifications/push`, `POST|DELETE
      /api/notifications/push/subscribe`, `PushCard.svelte`, and the sender
      hook in `src/notification_engine.rs`. `frontend/public/sw.js` now
      handles `tag`, `requireInteraction` for critical alerts and focuses an
      existing window on click.
      *Rules honoured:* VAPID P-256 pair generated once, private half sealed
      with the appliance key (`hv1:`) and the file `0600`, only the public key
      exposed; a subscription exists only after the browser permission and an
      explicit click; the endpoint must be HTTPS on 443 and resolve to a
      **public** address (SSRF guard), redirects are not followed; `p256dh` is
      a 65-byte uncompressed point and `auth` exactly 16 bytes; 10 devices per
      account; RFC 8291 `aes128gcm` on `ring` (ECDH P-256, HKDF-SHA256,
      AES-128-GCM) with the RFC Appendix A vector as the test; delivery is
      best-effort with a 24 h TTL, no retry queue, `404`/`410` removes the
      device, ten other failures remove it, `401`/`403` is treated as a key
      problem and keeps the device; every device is revocable by its owner and
      account deletion cascades.
      *GDPR:* title/message/link only, per-device encryption, the push service
      sees endpoint/timing/size but not content, consent and revocation are
      per device — all documented in the guide.
      Verified: `web_push/crypto.rs` 10/10 (RFC vector), `web_push/mod.rs`
      6/6, `web_push_integration.rs` 10/10, E2E `settings/web-push` 7/7 on
      Chromium **and** Firefox, `cargo test --lib` 2,538 passed / the same 5
      pre-existing failures, `npm run check` 0/0, build green. Live probe: key
      generation, 87-char public key, sealed `hv1:` private key, file `0600`.
      No live push service (see below). See `docs/guides/web-push.md`.

- [x] **D1 — Content index (full-text over file contents)** · **L** · shipped
      2026-11-06
      `src/content_index/` (`mod.rs`, `repo.rs`), migration `078`, `GET
      /api/search/content`, `GET|PUT /api/search/index`, `POST
      /api/search/index/run`, `DELETE /api/search/index`, `ContentIndexCard.svelte`,
      the ⌘K merge in `frontend/src/lib/stores/fileProvider.ts`, and the
      design note `docs/analysis/d1-content-index-design.md`.
      *Design constraints honoured:* opt-in per root from the appliance's own
      Explorer roots, enabling confirmed; text allowlist + name/glob and
      extension exclusions; hidden entries and symlinks skipped; 1 MiB per
      file, UTF-8/no-NUL only; 50 000 docs / 256 MiB / 16 roots capped and
      reported as `truncated`; results filtered by
      `explorer_roots_for_session` **before** the snippet is built; retention
      1–3650 days plus per-root pruning; "delete my index" wipes and disables;
      SQLite **FTS5** (no new daemon) with external-content triggers; queries
      quoted into at most 8 literals so FTS operators cannot widen them.
      Verified: `content_index/mod.rs` 14/14, migration-parser +1,
      `content_index_integration.rs` 9/9 (FTS triggers, retention, admin gate,
      refusals, delete, ACL), E2E `settings/content-index` 7/7 on Chromium
      **and** Firefox, `cargo test --lib` 2,553 passed / the same 5
      pre-existing failures, `npm run check` 0/0, build green. Live probe on a
      scratch share: 2 text files indexed (a NUL-byte `.txt` skipped),
      `watermelon` and `banana` both found with snippets, delete removed 2 and
      disabled the feature. See `docs/guides/content-index.md`.

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
      rows, **17 are shipped** (the local DNS server joined file tags, bond,
      VLAN and the SNMP agent on 2026-11-05; the reverse proxy and the PWA push
      half on 2026-11-06; content search joined them with D1 on 2026-11-06) and
      1 is rejected (QuickConnect). **No open row remains.**
      **The file is left untouched on purpose** — it is the dated input record,
      and §2 of the implementation plan is its reconciliation. It is untracked
      (repo rule: no standalone root `.md` in commits).

---

## 7. Suggested order

1. ~~**B5 + B6 together**~~ — shipped 2026-11-04.
2. ~~**B4** (SNMP agent)~~ — shipped 2026-11-04. Phase B is complete.
3. ~~**C2**~~ and ~~**C3**~~ — shipped 2026-11-06. Phase C is complete.
4. ~~**D2**~~ — shipped 2026-11-06. ~~**D1**~~ — shipped 2026-11-06 with its
   design note. **The list is complete.**

Phase B is complete (B1–B6). Phases C and D have no blocker between them. Each item ships with its guide,
`API.md` entry, `FeatureDoc`, unit + integration + E2E tests, and `TESTING.md`
counts, per the definition of done in `CLAUDE.md`.

---

## 8. Session state — 2026-11-06

What the Phase B session and this one shipped, and what they left behind, so the
next one starts from facts rather than from a document.

**Shipped**

| Commit | Item |
|---|---|
| `ffdd7f1` | B1 — private per-user file tags (+ two defects fixed: a tag on a deleted path could not be removed; the ⌘K coverage note was hidden at zero hits, which was making `desktop 6b.3` red on `main`) |
| `3df8a7d` | B5 + B6 — link aggregation and VLAN devices |
| `82b7c1e` | B4 — the local SNMP agent |
| C1 (2026-11-05) | the local DNS server, on the `dnsmasq` the DHCP card already drove — `src/local_dns.rs`, `GET|PUT /api/network/dns-server`, `LocalDnsCard.svelte`, `docs/guides/local-dns.md` |
| C2 (2026-11-06) | the generic reverse proxy — `src/reverse_proxy/`, migration `076`, `GET|POST|PUT|DELETE /api/proxy/rules`, `GET /proxy-auth`, `ReverseProxyCard.svelte`, guides `reverse-proxy.md` and `analysis/c2-reverse-proxy-design.md` |
| C3 (2026-11-06) | router port forwarding — `src/upnp.rs`, `GET /api/network/upnp`, `POST|DELETE /api/network/upnp/mappings`, `UpnpCard.svelte`, guide `upnp.md`, optional `miniupnpc` in the installer and dependency inventory |
| D2 (2026-11-06) | Web Push — `src/web_push/`, migration `077`, push endpoints, `PushCard.svelte`, sender in `notification_engine`, guide `web-push.md` |
| D1 (2026-11-06) | content index — `src/content_index/`, migration `078`, search/index endpoints, `ContentIndexCard.svelte`, ⌘K merge, guides `content-index.md` and `analysis/d1-content-index-design.md` |

**Verified on this session's machine**

- `cargo test --lib` — **2,553 passed, 5 failed** after D1 (the same five
  unrelated failures `TESTING.md` has tracked since Phase A:
  `checksum::…evicts…`, `settings`/`i18n` density ×2, `sse::progress` ×2 — all
  in files no session touched).
- `tests/local_dns_integration.rs` 14/14, `tests/dhcp_server_integration.rs`
  21/21, `tests/phase4_auth_integration.rs` 6/6,
  `tests/explorer_integration.rs` 99/99,
  `tests/global_search_integration.rs` 14/14,
  `tests/network_integration.rs` 28/28, `tests/snmp_integration.rs` 10/10.
- E2E, live server + scratch `DATA_DIRECTORY` (`PORT=8311`): `settings/local-dns`
  **12/12 on Chromium and 12/12 on Firefox**; `settings/dhcp` and
  `settings/snmp-agent` 13/13 on Chromium (13/13 on Firefox for the pair run
  before the C1 commit); C2 `settings/reverse-proxy` **11/11 on Chromium and
  11/11 on Firefox** against the real backend (rules cleaned up in `afterAll`),
  plus a live loopback probe of `open rule → upstream`, `302 → /proxy-auth →
  ticket → __hv_proxy_auth → hv_app_session → upstream`, and ticket replay
  `401`.
- C2 integration: `tests/reverse_proxy_integration.rs` **21/21** (CRUD, auth,
  every refusal, and a real listener against a loopback upstream);
  `reverse_proxy` unit **31/31**.
- C3 verification: `upnp_integration.rs` **9/9**, `upnp` unit **16/16** +
  handler **1/1**, E2E `settings/upnp` **9/9 on Chromium and 9/9 on Firefox**
  with every router call intercepted; dependency-inventory and English-only
  installer tests stay green with the new `miniupnpc` optional package.
- D2 verification: `web_push/crypto.rs` **10/10** (the RFC 8291 Appendix A
  vector byte-for-byte), `web_push/mod.rs` **6/6**,
  `web_push_integration.rs` **10/10**, E2E `settings/web-push` **7/7 on
  Chromium and 7/7 on Firefox** with the Push API scripted; live probe of
  `GET /api/notifications/push` on the scratch instance generated the keys
  (`hv1:` sealed, file 0600, 87-char public key).
- D1 verification: `content_index/mod.rs` **14/14**, the migration parser
  **+1**, `content_index_integration.rs` **9/9**, E2E
  `settings/content-index` **7/7 on Chromium and 7/7 on Firefox** with every
  call intercepted; live probe on a scratch share indexed 2 text files
  (binary skipped), searched two terms with snippets, then deleted 2 documents
  and turned the feature off.
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

**No roadmap item is left.** The next work is maintenance or a new roadmap,
not a queue item. Live debt still owed, unchanged: a browser on a second device
resolving a reverse-proxy rule name through the C1 DNS server, a TLS upstream
with a publicly trusted certificate, a real UPnP-IGD router, a real push
delivery through FCM/Mozilla/Apple, a multi-terabyte content-index run, and the
pre-existing network/E2E debt listed above (all need a disposable VM/LAN or a
real device).
