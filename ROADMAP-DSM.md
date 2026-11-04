# HomeVault — DSM parity roadmap

Last updated: 2026-11-03 · Baseline: HomeVault `main` @ `adf0132` vs Synology
DSM 7.4.1 (build 90080, DS925+)

One-line purpose: the capability gaps between HomeVault and DSM that are worth
closing, phased, with the evidence, the implementation sketch and the
acceptance criteria needed to pick any item up cold.

> **This is not `ROADMAP.md`.** That file tracks *defects and debt* found by
> auditing the current tree, each with a `path:line` and a verdict. This file
> tracks *features HomeVault does not have yet*, derived from a reference
> product. Nothing here is committed scope; an item becomes work only when it
> moves to `In progress` and gets a guide.

## Document status

| Field | Value |
|---|---|
| Status | **Phases 0–4 complete; Phase B shipped (6 of 6).** Every quick win QW-01…QW-13 delivered. Phase 0 closed 2026-11-02 (P0.1: six E2E suites green on Firefox **and** Chromium; P0.2: capability contract, 32 `400`s converted to `503`). Phase 4 hardened by Phase A the same day (22 unauthenticated routes fixed). Phase B: **B2 ISO mounting** (`58fe84b`) and **B3 SMS alerts** (`8e33dd4`) shipped 2026-11-02; B1 (file tags) shipped 2026-11-03, verified live on Chromium and Firefox; B4 (SNMP agent), B5 (link aggregation) and B6 (VLAN) shipped 2026-11-04; **C1 (local DNS server) shipped 2026-11-05** on the `dnsmasq` the DHCP card already drove (one daemon, one drop-in, one start/stop gate); **C2 (generic reverse proxy) shipped 2026-11-06** — one host-routed listener, private-address-only upstreams and a cross-host one-time-ticket gate; **C3 (UPnP-IGD port forwarding) shipped the same day** through `upnpc`, read-only first and reversible; **D2 (Web Push backend) shipped 2026-11-06** — VAPID keys sealed at rest, per-device consent, RFC 8291 encryption and a sender in the notification engine; **D1 (content index) shipped the same day** — opt-in FTS5 over chosen Explorer roots, ACL-filtered results, bounded and deletable. **Phases B, C and D are complete: every roadmap item is shipped.** |
| Reference product | DSM 7.4.1-90080 (Synology DS925+ pat file, extracted) |
| HomeVault baseline | `main` @ `adf0132` — 75 migrations, 73 guides, 37 screens, 91 integration suites, 2,447 lib tests passing (the same 5 pre-existing failures unrelated to this roadmap, listed in `TESTING.md`) |
| Method | Static comparison: DSM UI modules, `SYNO.*` API libraries and bundled packages vs HomeVault source, installer package groups and screens |
| Owner | unassigned |
| Quick wins delivered | **QW-01 through QW-04** have documented green verification. **QW-05, QW-06, QW-07, QW-08, QW-09 and QW-11** are implemented and Phase 0 verified by unit, integration and dedicated E2E coverage. The final Firefox scratch pass ran **70 passed / 2 skipped / 0 failed** across the six dedicated suites. Delivered capabilities include SMB recycle bin unified with Explorer Trash, automatic retention/size-cap purge, reflink/instant copy, share-link password + download cap + one-time links, remote syslog forwarding, Wake-on-LAN (plus the scheduled-task "wake first" pre-step), persistent login auto-block with its request-rate row, connected-clients visibility and disconnect action, per-share size quotas, and full configuration backup/restore. Static extraction follow-up adds verified gaps and candidates below. **+2026-10-30: three reliability fixes shipped while verifying QW-03 live — see "Reliability fixes delivered" at the end.** |

## How the comparison was made

The DSM side is the extracted `.pat` of DSM 7.4.1-90080:

| DSM artefact | Count | Where |
|---|---|---|
| UI module directories (`webman/modules/`) | 58 | `usr/syno/synoman/webman/modules/` |
| `SYNO.*` API libraries (`webapi/*.lib`) | 127 | `usr/syno/synoman/webapi/` |
| Bundled packages (extracted package `INFO` manifests) | 19 | `packages_extracted/` — FileStation, SMBService, StorageManager, HyperBackup-ED, HighAvailability, ScsiTarget, DhcpServer, QuickConnect, USBCopy, SecureSignIn, SynologyUniversalSearch, … |
| System image | 925 MB / 18 852 files | `hda1/` |

The HomeVault side is the current working tree. Counts change frequently; consult
`SCREENS.md`, `docs/INDEX.md`, `installer/install.sh`, `catalog/` and
`sqlx/migrations/` for the current inventory. Feature claims below cite the
relevant source or guide where practical.

The comparison is **capability-level, not UI-level**. DSM exposes some of these
through a different screen, a different name or a separate package; what matters
here is whether a home user can do the job.

## Priority, status and effort

| Status | Meaning |
|---|---|
| **Idea** | Written down, not triaged |
| **Agreed** | Accepted into scope, no code yet |
| **In progress** | Has an owner and a branch |
| **Shipped** | Acceptance met, guide written, tests green |

| Effort | Order of magnitude |
|---|---|
| **S** | ≤ 2 days |
| **M** | 3–5 days |
| **L** | 2–3 weeks |
| **XL** | 4 weeks or more, or needs a subsystem |

Tranches are ordered by **user value ÷ effort**, not by DSM's own menu order.
The QW register later in this document ranks smaller enhancements and changes
that improve already-shipped capabilities.

## Gap summary

Refreshed 2026-11-02 against the working tree: 21 of the 23 rows below were
listed as unbuilt long after the code shipped, which made this table contradict
the detail sections further down. Only rows 16 and 17 remain open (both
deliberate), and the genuine remaining scope lives in the per-phase sections and
in `docs/analysis/dsm-parity-implementation-plan.md`.

| # | DSM capability | HomeVault today | Item |
|---|---|---|---|
| 1 | Let's Encrypt / ACME certificates | ACME issuance and renewal are implemented (`src/acme.rs`, `docs/guides/acme.md`) | Shipped |
| 2 | DDNS | DuckDNS, dynv6, Cloudflare and custom providers implemented (`src/ddns.rs`, `docs/guides/ddns.md`) | Shipped |
| 3 | Reachability advisor | Ordered reachability checks and fixes implemented (`src/reachability.rs`, `docs/guides/reachability.md`) | Shipped |
| 4 | Trusted devices, rescue email and passkeys | Trusted devices, rescue e-mail and WebAuthn/passkeys all implemented (`src/identity.rs`, `src/passkey.rs`, `docs/guides/identity.md`) | Shipped |
| 5 | Rsync server (`rsyncd`, per-share modules) | `src/rsync_server.rs` behind `homevault-rsync.service`, per-share modules, per-account passwords (`docs/guides/rsync.md`) | Shipped |
| 6 | Time Machine backups (SMB + Bonjour) | managed `vfs_fruit` block per share + `_adisk._tcp` advertisement (`docs/guides/shares.md`) | Shipped |
| 7 | iSCSI target (ScsiTarget package) | `src/iscsi.rs` — backstores, IQNs, portals, LUNs, ACLs, CHAP (`docs/guides/iscsi.md`) | Shipped |
| 8 | Wake-on-LAN | Saved machines, magic packet on UDP 9/7 over the interface that owns the target subnet, and a "wake first" pre-step on scheduled tasks (`src/wol.rs`, `docs/guides/power.md`) | Shipped (QW-09) |
| 9 | Remote syslog client | RFC 5424/3164 over UDP/TCP/TLS with a pinned CA, bounded drop-oldest queue (`src/syslog_forward.rs`, `docs/guides/journal.md`) | Shipped (QW-08) |
| 10 | Per-share / per-user bandwidth limits | per-service shaping with a weekly schedule and pause-all (`src/traffic_control.rs`, egress only) | Shipped (P3.2) |
| 11 | Snapshot replication to another NAS | `src/replication.rs` — incremental, resumable, local or SSH, destination-only retention (`docs/guides/replication.md`) | Shipped |
| 12 | Immutable / WriteOnce snapshots | `src/snapshot_locks.rs` — row **and** filesystem hold, break-glass release (`docs/guides/snapshots.md`) | Shipped |
| 13 | Backup target mode + disaster recovery runbook | `src/backup_target.rs` (append-only tenants) + `docs/guides/disaster-recovery.md` | Shipped |
| 14 | Configuration backup & restore | full appliance archive with optional secrets and passphrase (`src/config_backup.rs`, QW-11) | Shipped |
| 15 | AD / LDAP + SSO (OIDC, SAML) | `src/directory.rs` + `src/oidc.rs` + `src/saml.rs` (`docs/guides/directory-sso.md`) | Shipped (P3.1) |
| 16 | Photos / Video / Audio stations | Docker catalog (Immich, Jellyfin, Navidrome) — deliberate: integrate, do not rebuild | **Open (deliberate)** |
| 17 | Mobile apps, push notifications | installable PWA (`manifest.json` + `sw.js`, which already handles `push`); **no VAPID keys, no subscription, no sender** | **Open** |
| 18 | Security Advisor (hardening audit) | `src/security_advisor.rs` — unified read-only checklist with severity, evidence and score (`docs/guides/health.md`) | Shipped (QW-10) |
| 19 | Fan control / thermal curves | `src/fan_control.rs` — hwmon PWM, profiles, 20 % floor, failsafe (`docs/guides/fan-control.md`) | Shipped (P4.4) |
| 20 | UPS server mode (network UPS) + orderly shutdown | `src/ups_server.rs` — client allowlist, five-stage policy, `skipped` reported (`docs/guides/ups-server.md`) | Shipped (P4.1) |
| 21 | DHCP / TFTP / PXE server | `src/dhcp_server.rs` — dnsmasq, ranges, reservations, leases (`docs/guides/dhcp.md`) | Shipped (P4.3) |
| 21b | Local DNS server | `src/local_dns.rs` — local A/alias records, answered domains, upstream forwarders, LAN advertisement; the *same* `dnsmasq` as row 21 (`docs/guides/local-dns.md`) | Shipped (C1, 2026-11-05) |
| 22 | USB one-touch copy | `src/usb_copy.rs` — UUID-matched profiles, checksums, safe eject (`docs/guides/usb-copy.md`) | Shipped (P4.2) |
| 23 | SHR-style guided redundancy | `src/guided_redundancy.rs` — layout suggestions with capacity/efficiency/risk (`docs/guides/guided-redundancy.md`) | Shipped (P4.5) |

---

## T1 — Reachability and identity (do first)

ACME, DDNS, reachability checks, trusted devices and rescue e-mail have shipped
in the current working tree. WebAuthn/passkeys remain a possible identity
improvement. HomeVault deliberately avoids a Synology-operated relay (see
[Non-goals](#non-goals-and-deliberate-divergences)).

### DSM-01 — ACME certificate automation (Let's Encrypt) — shipped

**DSM reference**: `SYNO.Core.Certificate`; Control Panel → Security →
Certificate issues, renews and swaps Let's Encrypt certificates, including
DNS-01 for wildcard names.

**Delivered scope**: ACME issuance and renewal, provider/challenge handling,
private key storage and Settings integration are implemented. See
`src/acme.rs`, `src/acme_worker.rs`, `src/tls.rs` and `docs/guides/acme.md`.
This item is retained as DSM traceability, not future scope.

### DSM-02 — DDNS client — shipped

**DSM reference**: `SYNO.Core.DDNS`; Control Panel → External Access → DDNS
supports Synology, DynDNS, Cloudflare, OVH, No-IP, DuckDNS, custom URL.

**Delivered scope**: DuckDNS, dynv6, Cloudflare and custom providers with
change-aware updates and Settings integration. See `src/ddns.rs`,
`src/handlers/ddns.rs` and `docs/guides/ddns.md`. This item is retained as DSM
traceability, not future scope.

### DSM-03 — Reachability advisor — shipped

**DSM reference**: Control Panel → External Access → Router Configuration
(UPnP port mapping), plus DSM's own reachability test.

**Delivered scope**: ordered reachability checks and remediation guidance are
implemented. See `src/reachability.rs`, `src/handlers/reachability.rs` and
`docs/guides/reachability.md`. Do not infer automatic UPnP/NAT-PMP mapping from
the presence of this advisor; consult the guide for its actual behavior.

### DSM-04 — Passkeys, trusted devices and rescue email

**DSM reference**: `SYNO.SecureSignIn` (bundled), `SYNO.Core.TrustDevice`,
`SYNO.Auth.RescueEmail`; DSM 7.x supports hardware security keys / passkeys and
"remember this device".

**HomeVault today**: password + TOTP, offline recovery codes, trusted devices,
verified rescue e-mail and account recovery are implemented (`src/identity.rs`,
`docs/guides/identity.md`). **Remaining idea**: WebAuthn/passkeys using
`webauthn-rs`, with TOTP fallback and phishing-resistant sign-in. Keep it
separate from the shipped recovery flows and do not weaken offline recovery.

**Potential acceptance**: passkey registration and sign-in work with a software
authenticator; credential revocation is available to the owner; TOTP remains a
fallback; recovery continues to use the existing protected recovery paths.
**Potential effort**: L. **Risk**: medium — the WebAuthn RP-ID must match the
configured hostname.

> **✅ Delivered 2026-10-31** as Phase 1 **P1.3** — see that entry. Every
> potential-acceptance line above is covered: registration and sign-in work
> against a software authenticator (`tests/passkey_integration.rs`), the owner can
> rename and revoke, TOTP and the offline recovery paths are untouched, and the
> RP-ID/origin risk is handled by deriving both from configuration and showing
> the origin on the card.

---

## T2 — Protocol interop

These items do not add new concepts; they make HomeVault a first-class citizen
of networks that already contain Macs, Linux boxes and backup scripts.

### DSM-05 — Rsync server

**DSM reference**: `SYNO.Core.FileServ.Rsync`; Control Panel → File Services →
Rsync, with per-share "Rsync backup service" and an account list.

**HomeVault today**: `rsync` is installed (`installer/install.sh`) but only used
internally by the cache mover (`src/cache.rs:59`). There is no daemon, no module
per share, no rsync-over-SSH path.

**Proposal**: generate `/etc/rsyncd.conf` from shares flagged `rsync = true`
(same generator pattern and validation rules as `smb.conf` in `src/shares.rs`),
run a dedicated `rsyncd` instance on its own port under a systemd unit managed
like the SFTP sshd (`src/sftp.rs`), with `auth users` mapped to HomeVault users
and secrets in `/etc/homevault/rsyncd.secrets` (`0600`). Also expose
"rsync over SSH" as a documented path using the SFTP sshd already running.

**Acceptance**: one module per flagged share; `rsync --list-only` and a real
transfer succeed as a normal user; a share rename/delete rewrites the config and
reloads the daemon; paths cannot escape the share root (`..`, symlinks).

**Tests**: unit (config generation, name validation, escape attempts),
integration (real `rsync` against a temp module), Playwright (Shares → Rsync
tab). **Docs**: `docs/guides/shares.md` + new `docs/guides/rsync.md`.
**Effort**: M.

### DSM-06 — Time Machine support

**DSM reference**: Control Panel → File Services → SMB → Advanced → "Enable
Time Machine support"; DSM also advertises the volume over Bonjour.

**HomeVault today**: Samba is generated from `src/shares.rs` with plain
options; no `vfs_fruit`, no `fruit:time machine`, no `_adisk._tcp` record
(`src/mdns.rs` publishes the host and shares, not a Time Machine volume).

**Proposal**: a per-share "Time Machine" toggle that adds
`vfs objects = fruit streams_xattr`, `fruit:time machine = yes`, a quota
(`fruit:time machine max size`) and `fruit:metadata = stream`; publish
`_adisk._tcp` via `src/mdns.rs` with the `dk0`/`adVF` records macOS expects;
require SMB signing settings to stay compatible.

**Acceptance**: a real macOS client sees the share in Time Machine preferences,
completes a first backup and a subsequent incremental one, and the quota is
enforced; disabling the toggle removes both the Samba options and the Bonjour
record.

**Tests**: unit (config fragment, mdns TXT records), integration (config
reload, quota string), Playwright (Shares → SMB tab). **Docs**:
`docs/guides/shares.md`, `docs/guides/mdns.md`. **Effort**: M.
**Note**: AFP is deliberately not implemented — Apple deprecated it; SMB is the
supported Time Machine transport.

### DSM-07 — iSCSI target (optional, opt-in)

**DSM reference**: `ScsiTarget` package (bundled in this pat) — LUNs, targets,
CHAP, thin provisioning, snapshots.

**HomeVault today**: none; block storage is limited to local volumes.

**Proposal**: wrap `targetcli`/LIO (kernel) rather than `tgt`, behind a
feature flag that the installer only offers on x86_64/aarch64 with the
`target_core_mod` modules present. Model: backstore (file or ZFS zvol) → LUN →
target with portal, initiator ACL and optional CHAP. Every mutation is a
root-scoped command through the existing sudoers pattern (`src/setup.rs`), with
a bounded timeout, and the UI shows the exact command it will run.

**Acceptance**: a Linux initiator can log in, see the LUN and write to it; CHAP
is enforced when enabled; deleting a target never deletes the backstore without
a second confirmation; the feature is invisible when the kernel lacks the
modules.

**Tests**: unit (config generation, CHAP masking in logs), integration (LIO
against a loop file, skipped when unavailable), Playwright (Storage → iSCSI).
**Docs**: new `docs/guides/iscsi.md`. **Effort**: XL. **Risk**: high — kernel
modules, root commands, data-loss potential. Ship last in T2, or defer.

### DSM-08 — Wake-on-LAN

**DSM reference**: Control Panel → Hardware & Power → WOL, plus "Wake on LAN"
from DSM's own tools.

**HomeVault today**: `src/power.rs` can reboot, shut down and suspend; it cannot
wake a machine.

**Proposal**: store per-host MAC addresses (a small `wol_hosts` table) and send
a magic packet from the server (UDP 9 and 7, broadcast on the selected
interface). Add a "Wake before this task" option to scheduled tasks (a backup or
a sync at 03:00 wakes the NAS first) and to the server-control screen for a
remote host. This is useful precisely because HomeVault is often installed on a
machine that sleeps.

**Acceptance**: a powered-off host on the same L2 segment boots from the UI; the
packet is sent on the interface that owns the target subnet; failure says
"sent, not confirmed" rather than claiming success.

**Tests**: unit (packet bytes, interface selection), integration (UDP listener
captures the magic packet), Playwright (Settings → Power → Wake host).
**Docs**: `docs/guides/power.md`. **Effort**: S.

**Delivered 2026-10-31** as **QW-09** — see that entry for the delivered scope,
the deviations (the interface inventory is read from `/sys/class/net`, never a
subprocess) and the test counts. The card lives in Settings → **System**, next
to the power controls, rather than in a separate Power screen.

### DSM-09 — Remote syslog client

**DSM reference**: `SYNO.Core.SyslogClient`; Log Center → Log Sending, with
RFC 5424/3164, TCP/UDP/TLS and per-category filters.

**HomeVault today**: the Journal reads local sources
(`/var/log/syslog`, auth, kern, daemon — `src/handlers/journal.rs`); nothing is
forwarded.

**Proposal**: `src/syslog_forward.rs` with a settings card (destination,
protocol, format, facility, TLS with optional CA pinning, queue on failure), a
bounded in-memory queue drained by a background task, and forwarding of
HomeVault's own audit events in addition to the system sources. Never block a
request on a syslog write.

**Acceptance**: a `logger`-generated event arrives at a test collector in the
selected format; an unreachable collector drops oldest-first after the queue
cap and reports "degraded" instead of stalling; secrets in messages are not
forwarded (the existing redaction path is reused).

**Tests**: unit (RFC 5424 framing, queue bounds), integration (local UDP/TCP
collector), Playwright (Settings → Monitoring → Syslog).
**Docs**: `docs/guides/journal.md`. **Effort**: S.

**Delivered 2026-10-31** as **QW-08** — see that entry for the delivered scope,
the deviations (only HomeVault's own audit events are forwarded; the host's
`/var/log` sources are not re-shipped) and the test counts.

### DSM-10 — Bandwidth limits per share and per user

**DSM reference**: `SYNO.Core.Network.TrafficControl`, `SYNO.Core.BandwidthControl`;
per-service and per-user speed limits with a schedule.

**HomeVault today**: `src/transfer_quota.rs` counts bytes per user/group and
fails closed, but nothing shapes throughput. A single large copy can saturate a
1 GbE link and starve the rest of the household.

**Proposal**: traffic shaping on the egress interface (`tc` HTB classes per
share/user is overkill on a home box) — start with **per-service limits** (SMB,
NFS, FTP, WebDAV, SFTP) via `tc`/`ifb`, then per-user limits enforced in the
protocol servers where possible (Samba `smb.conf` has no per-user rate limit;
FTP/WebDAV/SFTP can). Provide a schedule (weekday/time ranges) and a "pause all
transfers" switch.

**Acceptance**: with a limit of 10 MB/s, an `smbclient get` measures ≤ 12 MB/s
and returns to full speed when disabled; the rule survives a reboot (unit +
persisted config); the UI shows the active rules and the interface they apply
to.

**Tests**: unit (tc command generation, schedule windows), integration (real
transfer rate against a shaped interface, skipped without `CAP_NET_ADMIN`),
Playwright (Network → Traffic control). **Docs**: `docs/guides/network.md`,
`docs/guides/transfer-quotas.md`. **Effort**: M. **Risk**: medium — shaping can
lock out the admin's own session; always exempt management traffic.

---

## T3 — Data protection

DSM's strongest area. HomeVault already has the primitives (Btrfs/ZFS
snapshots, restic); what is missing is the *off-box* story.

### DSM-11 — Snapshot replication

**DSM reference**: `Snapshot Replication` package — push/pull snapshots between
appliances, retention per destination, resume after interruption.

**HomeVault today**: `src/snapshots.rs` creates, deletes and rolls back local
ZFS/Btrfs snapshots (`src/snapshots.rs:34`); `src/restic.rs` backs up to a
repository. There is no snapshot-level replication to a second HomeVault.

**Proposal**: a replication job model (source pool/dataset, destination
`host:path` or `ssh://`, schedule, retention, bandwidth cap) executed with
`zfs send | ssh zfs receive` (incremental with a bookmark) or
`btrfs send | btrfs receive`, resumable (`-s` for ZFS), with a per-job journal
entry and a dry-run mode. The remote side can be a plain SSH account or another
HomeVault exposing a restricted "receive" endpoint.

**Acceptance**: a first full send followed by an incremental send reproduces
the source dataset byte-for-byte (`zfs diff`/`btrfs subvolume find-new` empty);
an interrupted transfer resumes without resending the whole stream; retention
deletes only destination snapshots created by the job.

**Tests**: unit (command construction, retention math), integration (loop-backed
ZFS/Btrfs pools, skipped when unavailable), Playwright (Backup → Replication).
**Docs**: new `docs/guides/replication.md`. **Effort**: L. **Depends on**:
none, but pairs with DSM-12.

### DSM-12 — Immutable snapshots (retention lock)

**DSM reference**: immutable snapshots / WriteOnce shared folders — a snapshot
cannot be deleted before its expiry, even by an administrator.

**HomeVault today**: snapshots are admin-deletable; ransomware protection
(`src/ransomware.rs`) watches for suspicious writes but cannot protect the
snapshots themselves.

**Proposal**: on ZFS use `zfs hold` with a tag HomeVault controls; on Btrfs mark
the subvolume read-only (`btrfs property set ro true`) and keep the lock list in
SQLite. Deletion requires the hold to be released, which the API refuses before
`locked_until` unless a documented break-glass procedure (typed confirmation +
journal entry) is used. Apply the same lock to restic snapshots via
`restic forget --keep-*` exclusions.

**Acceptance**: an attempt to delete a locked snapshot through the API, the UI
or a direct `zfs destroy` fails before expiry; the lock list survives a restart;
break-glass is journalled with the acting user.

**Tests**: unit (lock state machine), integration (real `zfs hold` / `btrfs ro`),
Playwright (Storage → Snapshots → lock badge). **Docs**:
`docs/guides/snapshots.md`, `docs/guides/ransomware.md`. **Effort**: L.

### DSM-13 — Backup target mode and DR runbook

**DSM reference**: Hyper Backup Vault (a NAS can be a backup destination),
`SYNO.DisasterRecovery`, `SYNO.S2S`.

**HomeVault today**: restic repositories can live anywhere, including an SSH
target, but HomeVault cannot *be* the destination with quotas, per-tenant
isolation and an append-only guarantee.

**Proposal**: a "Receive backups" role: create a system user per source, an
append-only restic repository per tenant (`restic` supports `--append-only` on
the server side via `rest-server`), a quota, and a restore-in-place flow that
never overwrites a live share without an explicit target. Document the DR
runbook (bare metal → install → attach data disk → restore) in a guide, and add
a "test restore" scheduled task that restores one random file into a scratch
directory and verifies its hash.

**Acceptance**: a second HomeVault can push backups to the first with a token;
the destination enforces the quota and refuses `forget`/`prune` from the source;
a scheduled test restore proves the repository is readable.

**Tests**: unit (tenant isolation, quota math), integration (two instances,
rest-server), Playwright (Backup → Destinations). **Docs**:
`docs/guides/backup-restic.md` + new `docs/guides/disaster-recovery.md`.
**Effort**: L.

### DSM-14 — Configuration backup and restore

**DSM reference**: `SYNO.Core.ConfigBackup`; Control Panel → Update & Restore →
Configuration Backup, with optional encryption and selectable content.

**HomeVault today**: `src/db_backup.rs` dumps the SQLite database before a
migration; there is no user-facing export/import of settings, users, shares and
ACLs.

**Proposal**: export a signed, optionally encrypted archive containing the
database (consistent snapshot via `VACUUM INTO`), the generated configs
(`smb.conf`, `/etc/exports`, netplan, sshd for SFTP, ufw rules), and a manifest
with versions. Secrets (password hashes, 2FA keys, tokens, private keys) are
**excluded by default** and included only behind an explicit "include secrets"
choice with a passphrase. Import validates the schema version and refuses to
downgrade.

**Acceptance**: export → wipe test instance → import restores users, groups,
shares and settings; a wrong passphrase fails cleanly; importing a newer schema
is refused with a clear message; the archive never contains a plaintext secret.

**Tests**: unit (manifest, redaction), integration (round-trip on a temp data
dir), Playwright (Settings → System → Backup configuration). **Docs**:
`docs/guides/settings.md`, `docs/guides/backup-restic.md`. **Effort**: M.

### DSM-15 — Directory services (LDAP/AD, then SSO)

**DSM reference**: `SYNO.Core.Directory.LDAP`, `SYNO.Core.Directory.SSO`,
`.SAML`, `.OIDC`, `.CAS`, Azure/Entra SSO libraries; Domain/LDAP join with
users and groups imported.

**HomeVault today**: local accounts only (`src/users.rs`, `src/groups.rs`);
NSS/LDAP appears only in comments about `getent` hanging
(`src/groups.rs:54`, `src/auth/service.rs:737`).

**Proposal**, three steps, each shippable:

1. **LDAP/AD client** (L): configure `sssd` (or `nslcd`) through a managed
   drop-in, test the bind, map remote groups to HomeVault roles, and cache
   identity lookups so a dead directory does not block the login path.
2. **OIDC** (M): authorization-code + PKCE against a generic provider, with
   claim→user mapping, JIT provisioning off by default, and an admin-only
   "break-glass" local account.
3. **SAML** (M): SP metadata endpoint, signed assertions, same mapping.

Every remote identity is a **separate principal** from the local one: no silent
account merging, and removing the directory link never deletes local users.

**Acceptance**: a directory user can sign in and sees only their shares; a
directory outage fails open to local accounts, never to a blank screen; group
changes propagate within one login (no stale admin); the break-glass account
cannot be disabled by the directory.

**Tests**: unit (claim mapping, group→role), integration (OpenLDAP container,
mock OIDC/SAML IdP), Playwright (Settings → Users → Directory).
**Docs**: new `docs/guides/directory.md`, `docs/guides/sso.md`.
**Effort**: L + M + M. **Risk**: high — identity is the one place where a bug is
an outage.

---

## T4 — Family apps and mobile

DSM ships a media suite. HomeVault's stated strategy is *integration* (Docker
catalog + reverse proxy) rather than reimplementation; this tranche makes that
strategy actually pleasant, and adds the one thing the browser cannot do alone:
push notifications.

### DSM-16 — Media and photos: integrate deeply, do not rebuild

**DSM reference**: `SYNO.Core.MediaIndexing`, `SYNO.Core.PhotoViewer`,
`SYNO.VideoPlayer`, `SYNO.AudioPlayer`; Photo Station / Video Station / Audio
Station packages.

**HomeVault today**: the Explorer browses and previews files but has no media
index, no timeline and no transcoding; `catalog/` already offers Immich,
Jellyfin and Navidrome, and `src/app_proxy/` reverse-proxies them with an
optional session gate.

**Proposal** (chosen over building a media server):

1. **Curated app presets** (M): each catalog entry gains the volumes, environment
   and proxy path a home user needs (photos library → `/photos`, media library →
   `/media`), plus a "Open" tile on the desktop once installed. Immich and
   Jellyfin stop being generic containers and become *the* Photos and Media apps.
2. **Explorer media previews** (M): inline `<video>`/`<audio>` with range
   requests and a thumbnail strip for images, reusing the existing thumbnail
   cache; no transcoding.
3. **Optional index** (L, only if 1–2 are not enough): a background indexer
   (ffprobe + EXIF) writing `media_items`, powering search ("videos from 2024")
   and a read-only timeline in the Explorer.

**Acceptance**: after a one-click install, the app is reachable at
`/apps/<slug>/` with SSO through the existing gate, and its library points at a
HomeVault share without manual volume editing; a 4 GB video plays in the
Explorer with seeking (HTTP 206) and does not buffer the whole file into RAM.

**Tests**: unit (preset rendering, path validation), integration (proxy + range
request), Playwright (Apps Center → install → open, Explorer preview).
**Docs**: `docs/guides/apps-center.md`, `docs/guides/explorer.md`.
**Effort**: M + M (+ L optional).

### DSM-17 — Mobile: installable PWA and Web Push

**DSM reference**: DS file / DS photo / Synology Photos apps; `SYNO.DSM.PushNotification`.

**HomeVault today**: responsive Svelte UI, in-app notification center, email and
webhook channels (`src/notification_engine.rs`); no installable app, no push
when the browser is closed.

**Proposal**: a web app manifest + service worker (installable on Android/iOS,
offline shell, cached app icons), and Web Push (VAPID) as a fourth notification
channel with per-user subscriptions, a test button and automatic pruning of
expired endpoints. Add a share-target so "Share → HomeVault" uploads to a chosen
folder.

**Acceptance**: the app installs on Android and iOS home screens and opens
without a browser chrome; a backup failure delivers a push with the app closed;
a revoked subscription disappears from the list; no notification content is
sent to a push service beyond an opaque payload (title/body encrypted per
RFC 8291).

**Tests**: unit (VAPID key handling, payload encryption), integration (mock push
endpoint), Playwright (install prompt, subscription flow).
**Docs**: `docs/guides/notifications.md` + new `docs/guides/pwa.md`.
**Effort**: M.

---

## T5 — Appliance polish

Cheap, visible items that make HomeVault feel like an appliance rather than a
web app on a PC.

### DSM-18 — Security Advisor

> **✅ Shipped 2026-10-31** as **QW-10** — see that entry for the delivered
> scope, the deviations (English sentences rather than locale-dependent keys;
> no persisted score table) and the test counts. The card lives at the top of
> the Health screen, not on a separate Advisor screen.

**DSM reference**: `SYNO.SecurityAdvisor` — a checklist that scans settings and
returns pass/warn/fail with a one-click fix.

**HomeVault today**: Health, Firewall, Antivirus, Ransomware and TLS screens each
show their own state; nothing aggregates "is this box safely configured?".

**Proposal**: a read-only audit engine (`src/security_advisor.rs`) with checks:
admin account present and not `admin`, 2FA on admins, TLS valid and not
expiring, ACME on, firewall enabled with the expected ports, SSH password login
state for the SFTP instance, SMB signing, guest access off, snapshots enabled on
data pools, backups configured and last run successful, updates applied, no
share world-writable. Each finding carries severity, evidence and a deep link to
the fixing screen; the score is stored per run for the dashboard.

**Acceptance**: a fresh install scores what the code says it should (fixture
test); each check has one positive and one negative integration case; a fixed
finding disappears on the next run without a restart.

**Tests**: unit (each check, both ways), integration (endpoint + auth),
Playwright (Health → Advisor). **Docs**: `docs/guides/health.md`.
**Effort**: M.

### DSM-19 — Fan control and thermal curves

**DSM reference**: Control Panel → Hardware & Power → Fan speed mode (quiet /
cool / full), on supported models.

**HomeVault today**: temperatures are read (`src/health.rs`, Dashboard widget),
but `/sys/class/hwmon/*/pwm*` is never written.

**Proposal**: detect writable `pwmN` channels, expose a per-channel mode
(auto / manual duty / curve), apply a curve (temperature → duty with hysteresis)
from a small daemon loop, restore `auto` on shutdown, and refuse to run on
laptops where the EC owns the fan. Every write is logged once per change, not
per tick.

**Acceptance**: on a supported host the duty cycle follows the curve with
hysteresis and no oscillation; setting `auto` returns control to the firmware;
on a host without writable channels the card is hidden.

**Tests**: unit (curve math, hysteresis, clamping), integration (fake hwmon tree
via a temp root), Playwright (Settings → Monitoring → Fans).
**Docs**: `docs/guides/health.md`, `docs/guides/power.md`. **Effort**: M.
**Risk**: medium — wrong PWM values can cook or silence a box; hard limits and a
"restore auto" watchdog are mandatory.

### DSM-20 — UPS server mode

**DSM reference**: Control Panel → Hardware & Power → UPS → "Enable network UPS
server", plus "safe mode" shutdown ordering.

**HomeVault today**: `src/ups.rs` reads NUT variables as a **client**; there is
no way to serve UPS state to other machines, and shutdown ordering for VMs and
containers is not modelled.

**Proposal**: an opt-in `upsmon`/`upsd` server mode (bind to the LAN, allow-list
of clients, optional auth), and a shutdown plan: stop containers → shut down
VMs → flush restic → stop shares → power off, each step with a timeout and a
journal entry. The plan is previewable and dry-runnable.

**Acceptance**: a second machine's `upsc` reads the UPS through HomeVault; on a
simulated `OB LB` event the plan runs in order and a hung VM does not block the
final poweroff beyond its timeout; aborting the event (power returns) cancels the
plan before the destructive steps.

**Tests**: unit (plan state machine, timeouts), integration (NUT dummy driver),
Playwright (Settings → Power → UPS). **Docs**: `docs/guides/power.md`.
**Effort**: M.

### DSM-21 — DHCP / TFTP (later, opt-in)

**DSM reference**: `DhcpServer` and `SYNO.Core.TFTP`; useful for PXE booting and
for networks without a router that hands out reservations.

**HomeVault today**: none, and the Network screen explicitly manages client
interfaces.

**Proposal**: only if requested — `dnsmasq` for DHCP (with reservations and a
range) and `tftpd-hpa` for TFTP, both opt-in, both refusing to start when
another DHCP server answers on the segment, both with a "this will take over
addressing on this interface" confirmation. **Effort**: M. **Status**: Idea —
low value for the target user, real risk of breaking the LAN.

### DSM-22 — USB one-touch copy

**DSM reference**: `USBCopy` package — a front-panel button copies an inserted
USB disk to a share, and ejects it.

**HomeVault today**: removable disks are listed in Storage; copying is manual
through the Explorer.

**Proposal**: a udev rule (installed by `installer/install.sh`) that notifies
HomeVault of an inserted block device, plus a "Copy to" rule per device label
(UUID-matched, never by `/dev/sdX`), progress in the activity taskbar, checksum
verification and a safe-eject at the end. Hardware button support is out of
scope; insertion is the trigger.

**Acceptance**: inserting a labelled disk starts exactly one job; an interrupted
copy resumes or restarts without duplicating files; the disk is unmounted
cleanly at the end; a disk that is also a pool member is never touched.

**Tests**: unit (rule matching by UUID, ignore list), integration (loop device +
udev simulation), Playwright (Storage → Removable → rule).
**Docs**: `docs/guides/storage.md`. **Effort**: M.

### DSM-23 — Guided redundancy profiles (SHR-like)

**DSM reference**: Synology Hybrid RAID — automatic layout that mixes disk sizes
and tolerates one or two failures, with a "recommended" default.

**HomeVault today**: the RAID wizard creates mdadm/ZFS/Btrfs arrays with explicit
levels (`docs/guides/raid.md`); the user must know RAID 1 vs 5 vs 10, and mixed
disk sizes are their problem.

**Proposal**: a **recommendation layer**, not a new format: given the selected
disks, propose up to three layouts (max capacity with 1-disk redundancy, max
redundancy, max speed) computed from real sizes, explain the usable capacity and
the rebuild risk in one sentence each, and pre-fill the existing wizard. No
proprietary metadata, no silent reshaping.

**Acceptance**: for a fixture set of mixed disks the proposals match the
documented arithmetic; choosing one pre-fills level, devices and chunk size;
the user can still switch to the raw wizard at any step.

**Tests**: unit (capacity math for each level and size mix), integration
(wizard create path unchanged), Playwright (RAID → recommendations).
**Docs**: `docs/guides/raid.md`. **Effort**: S.

---

## Sequencing

| Tranche | Items | Theme | Indicative window |
|---|---|---|---|
| T1 | DSM-01…04 | Reachability and identity | weeks 1–6 |
| T2 | DSM-05…10 | Protocol interop | weeks 5–14 |
| T3 | DSM-11…15 | Data protection | weeks 12–24 |
| T4 | DSM-16…17 | Family apps and mobile | weeks 20–30 |
| T5 | DSM-18…23 | Appliance polish | interleaved, one per week |

Dependency notes:

- **DSM-02 → DSM-01** (DNS-01 provider reuse) and **DSM-01 → DSM-04** (WebAuthn
  RP-ID must match the final hostname).
- **DSM-05** and **DSM-06** share the Samba config generator and its validation
  rules; do them in one branch to avoid touching `src/shares.rs` twice.
- **DSM-11 → DSM-12** (replication carries the lock to the destination).
- **DSM-13** is most useful once **DSM-11** exists, but can ship independently.
- **DSM-15** step 1 (LDAP) is a prerequisite for step 2/3 if remote groups must
  map to roles.
- **DSM-07** (iSCSI) is the only XL item and the only one that can be dropped
  without affecting the rest.

> ## Where the current work is tracked
>
> This file is the capability ledger: it records, per DSM capability, what
> HomeVault has, what it lacks and why. It is **not** the working checklist —
> that is [`TASKLIST NICOLAS.md`](./TASKLIST%20NICOLAS.md), which lists the 9 open
> items with their phase, effort and order.
>
> Where each kind of information lives:
>
> | Question | File |
> |---|---|
> | What does HomeVault have vs DSM, and why is each gap closed or kept? | this file |
> | What may I pick up right now, in what order? | `TASKLIST NICOLAS.md` |
> | How was the gap list derived, and what did the source document get wrong? | `docs/analysis/dsm-parity-implementation-plan.md` |
> | Is feature X verified, and at which level? | `TESTING.md` |
> | How does feature X work? | `docs/guides/<area>.md` |

## Non-goals and deliberate divergences

These are DSM features HomeVault should **not** copy, with the reason:

| DSM feature | Why not | What HomeVault does instead |
|---|---|---|
| QuickConnect relay | Needs Synology-operated infrastructure; a third-party relay contradicts "no cloud account, nothing leaves the machine" | DDNS + ACME + WireGuard or a user-chosen tunnel |
| AFP / Time Machine over AFP | Apple deprecated AFP | SMB with `vfs_fruit` (DSM-06) |
| SHR proprietary metadata | Ties disks to a vendor format, unreadable elsewhere | mdadm/ZFS/Btrfs + guided profiles (DSM-23) |
| Synology package ecosystem | Closed format, signed by the vendor | Docker catalog + Git app stores (`src/app_catalog/`) |
| Photos / Video / Audio Station clones | Years of work, worse than upstream | Curated Immich / Jellyfin / Navidrome presets (DSM-16) |
| Surveillance Station | Niche, camera-vendor specific | Frigate in the catalog |
| HA cluster / CMS / S2S | Multi-appliance enterprise features, outside the home target | Replication (DSM-11) + backup target (DSM-13) |
| Active Backup for Business agents | Windows/macOS agent development | restic on the client, HomeVault as destination (DSM-13) |
| DSM's `SYNO.*` API shape | Proprietary, versioned by the vendor | The documented REST contract in `docs/API.md` + MCP |
| Synology C2 / Hybrid Share / Active Insight | Vendor cloud, vendor account, telemetry egress | rclone to a user-chosen provider; the local notifications centre |
| Fibre Channel (SAN Manager) | Datacentre hardware, no home appliance exposes it | iSCSI over Ethernet (DSM-07) |
| L2TP/IPSec VPN | Obsolescent, weak compared with WireGuard | WireGuard + OpenVPN |
| Mail server, Chat, Kubernetes | Outside a NAS's job; each is a full product | Mattermost / Mailcow / k3s in the Docker catalog |

## Definition of done (applies to every item)

Per `CLAUDE.md` and `docs/INDEX.md`, an item is not shipped until:

1. **Backend**: Rust unit tests for the logic, integration tests for the
   endpoint (auth class, error paths, root-command mocking), and every external
   command behind a bounded timeout in `spawn_blocking`.
2. **Frontend**: the screen/route added to the desktop registry
   (`frontend/src/routes/Desktop.svelte`), Playwright coverage under
   `tests/e2e/<area>/`.
3. **Installer**: new system packages declared in `installer/install.sh` in the
   right group (required vs optional), with a runtime probe in
   `src/dependencies.rs` so a missing tool degrades instead of failing.
4. **Data**: a numbered migration (`sqlx/migrations/060_…`), append-only.
5. **i18n**: keys in all four locales (`src/i18n/{en,fr,de,es}.toml`).
6. **Docs**: the matching `docs/guides/*.md` updated **and** its `FeatureDoc` in
   `src/rag_indexers.rs`; `docs/INDEX.md` and `SCREENS.md` updated.
7. **Security**: any new root command is added to the sudoers scope in
   `src/setup.rs` with the narrowest possible wildcard, and secrets never reach
   logs, the journal or an API response.

## Risks and guardrails

| Risk | Guardrail |
|---|---|
| Exposing the box to the internet (T1) | Opt-in only, explicit confirmation, firewall state shown next to every mapping, reachability test before claiming success |
| Root command surface grows with T2/T3 | One sudoers entry per feature, argument validation, no shell interpolation, command shown in the UI |
| Data loss during replication/restore | Dry-run first, destination-only deletion, immutable snapshots (DSM-12), test restore (DSM-13) |
| Identity outage locking everyone out (DSM-15) | Break-glass local admin, directory lookups cached and time-bounded, local accounts never deleted |
| Hardware variance (DSM-19/20/22) | Feature hidden when the interface is absent, fake-sysfs tests, restore-to-auto watchdog |
| Scope creep towards "DSM clone" | The non-goals table above; each item must name the user job it enables |

## Open questions

1. **Relay or not?** If a zero-configuration remote access story is required,
   the only honest options are a self-hosted relay (new infrastructure) or
   depending on WireGuard/Tailscale. Decide before DSM-03 ships, because it
   changes the onboarding copy.
2. **iSCSI**: is there a real user for it on a home box, or is it enterprise
   envy? If no volunteer tester with real hardware exists, drop DSM-07.
3. **Directory services**: support `sssd` only, or also `nslcd`? `sssd` is the
   modern path but heavier on a 1 GB machine.
4. **Media**: is the optional index (DSM-16 step 3) worth it once Immich and
   Jellyfin are one click away?
5. **Traffic shaping**: kernel `tc` on the host, or per-protocol limits in the
   servers (simpler, less precise)? The answer decides DSM-10's scope.

## Traceability

| DSM evidence | Item |
|---|---|
| `webapi/SYNO.Core.Certificate.lib` | DSM-01 |
| `webapi/SYNO.Core.DDNS.lib` | DSM-02 |
| `webapi/SYNO.Core.PortForwarding.lib` | DSM-03 |
| `packages/SecureSignIn-*.spk`, `SYNO.Core.TrustDevice.lib`, `SYNO.Auth.RescueEmail.lib` | DSM-04 |
| `webapi/SYNO.Core.FileServ.Rsync.lib` | DSM-05 |
| `packages/SMBService-*.spk`, `SYNO.Core.FileServ.ServiceDiscovery.lib` | DSM-06 |
| `packages/ScsiTarget-*.spk` | DSM-07 |
| `SYNO.Core.Hardware.lib` (WOL section) | DSM-08 |
| `webapi/SYNO.Core.SyslogClient.lib` | DSM-09 |
| `SYNO.Core.Network.TrafficControl.lib`, `SYNO.Core.BandwidthControl.lib` | DSM-10 |
| Snapshot Replication (package, not bundled here) | DSM-11 |
| `SYNO.Core.VolEncKeepKey.lib`, immutable-snapshot UI strings | DSM-12 |
| `SYNO.DisasterRecovery.lib`, `SYNO.S2S.lib`, `SYNO.Core.Backup.ED.lib` | DSM-13 |
| `SYNO.Core.ConfigBackup` (module `ConfigBackup`) | DSM-14 |
| `SYNO.Core.Directory.LDAP.lib`, `.SSO.SAML.lib`, `.OIDC.SSO.lib`, `.SSO.CAS.lib` | DSM-15 |
| `SYNO.Core.MediaIndexing.lib`, `SYNO.Core.PhotoViewer.lib`, `SYNO.VideoPlayer.lib`, `SYNO.AudioPlayer.lib` | DSM-16 |
| `SYNO.DSM.PushNotification.lib`, `SYNO.Core.DSMNotify.lib` | DSM-17 |
| `webapi/SYNO.SecurityAdvisor.lib` | DSM-18 |
| Control Panel → Hardware & Power → Fan speed (module `SystemInfoApp`) | DSM-19 |
| Control Panel → Hardware & Power → UPS server mode | DSM-20 |
| `packages/DhcpServer-*.spk`, `SYNO.Core.TFTP.lib` | DSM-21 |
| `packages/USBCopy-*.spk`, `SYNO.Core.ExternalDevice.lib` | DSM-22 |
| Storage Manager → SHR (module `StorageManager`) | DSM-23 |
## Quick-win register

The QW items below supplement DSM-01…DSM-23 with ranked improvements and
smaller gaps established by this static pass. **QW-01, QW-02, QW-03, QW-04,
QW-05, QW-06, QW-07, QW-08, QW-09, QW-10, QW-11, QW-12 and QW-13 are implemented** (each
item carries its own *Delivered* note with the files, endpoints, test counts and
any deviation); QW-12 ships as **name search** — indexed content search remains a
possible future extension and is not claimed. **No quick win remains open.** This
document is the sole DSM comparison roadmap.

### QW-01 — SMB Recycle Bin unified with the Explorer trash

> **✅ Shipped 2026-10-29.** A share recycles when its `smb.conf` `[section]`
> carries a marked `# homevault-recycle:begin … end` block
> (`smb_recycle_block` / `apply_recycle_to_section` in `src/shares.rs`) whose
> `vfs objects = recycle` repository is `.homevault-trash` — the exact folder
> the Explorer Trash already reads. Enable/disable rewrites only that block and
> reloads `smbd`; the on/off state is derived from the config file
> (`SharesService::get_smb_recycle_map`), never a second stored flag. Trash
> roots are canonical in the new `src/trash.rs` (`root_for` / `roots_from`),
> which adds every configured SMB share's recycle folder so a network delete is
> listed and restorable from the web Trash for every user. APIs:
> `GET /shares/smb/recycle`, `PUT /shares/{name}/smb/recycle`; the Shares → SMB
> tab shows one toggle per SMB share. *Deviation from the proposal below:* the
> state is a marked block in the share section rather than a `Share.options`
> entry, and the repository is one `.homevault-trash` per share (not `/%U`), so
> network and web deletes converge on one Trash screen.

**DSM reference**: `SYNO.Core.RecycleBin` (`.lib` manifest below); Control Panel
→ Shared Folder → *Enable Recycle Bin* with an optional "empty after N days";
the daemons `synounlinkd`/`recyclebind` and `logrotate.d/synorecycle` implement
it. Deletions over SMB/NFS land in a `#recycle` folder inside the share, not in
oblivion.

```
{"SYNO.Core.RecycleBin": {... "methods": {"1": [{"start": ...},
 {"stop": ...}, {"get": ...}]}}}
```

**HomeVault today**: the Explorer (web) has a working trash — `.homevault-trash`
at the top of each allowed root, `<uuid>_<name>` payloads plus a `.trashinfo`
sidecar carrying `Path=`/`DeletionDate=` (`src/browser.rs:35`, `:719`,
`:759`), with restore / purge / empty endpoints (`src/handlers/explorer.rs:3476`
`handle_explorer_trash`, `:3559` restore, `:3597` purge, `:3636` empty`). But
the SMB share section has **no** recycling — a delete from Windows/macOS is
permanent:

```rust
// src/shares.rs:590
fn smb_share_section(name: &str, comment: &str, path: &str) -> String {
    format!("\n[{name}]\n  comment = {comment}\n  path = {path}\n
             browseable = yes\n  read only = no\n  create mask = 0755\n")
}
```

**Why it wins**: accidental deletion is the #1 data-loss cause on a home NAS,
and the recovery UI already exists. This is the highest-value `S` item on the
list — it turns an existing feature into one that also protects network clients.

**Proposal**

1. Add a per-share `recycle` flag to the share model (`src/shares.rs:84`
   `Share.options` can carry it without a schema change) and surface a toggle
   in Shares → the share's SMB tab.
2. When set, emit the Samba fragment in `smb_share_section`:
   `vfs objects = recycle`, `recycle:repository = .homevault-trash/%U` (or a
   `#recycle` alias), `recycle:keeptree = yes`, `recycle:versions = yes`,
   `recycle:touch = yes`, `recycle:maxsize = 0`; hide the folder from normal
   listings with `veto files = /.homevault-trash/` + `delete veto files = no`.
3. Extend `FileBrowser::list_trash` (`src/browser.rs:759`) so a raw entry with
   no `.trashinfo` sidecar is still listed (name + mtime), and `restore` is
   best-effort using `keeptree`'s preserved relative path where the sidecar is
   absent. HomeVault-trashed and SMB-trashed items then share **one** Trash
   screen.
4. Apply the same rule to the share generator the moment the config is
   rewritten (create/rename/delete already rewrite `smb.conf`, `src/shares.rs:811`).

**Acceptance**

- Deleting a file from a real SMB client with recycling on leaves a recoverable
  copy; the Explorer → Trash screen lists it and restores it to its original
  folder (or the closest recoverable location for a tree-preserved entry).
- Turning the toggle off makes the next delete permanent and removes the VFS
  fragment on the next config write; `smb.conf` is never left with a dangling
  `vfs objects` line.
- A share whose path escapes its root (`..`, symlink) cannot write outside it
  (reuse the existing name validation, `src/shares.rs:401`).

**Tests**: unit (`smb_share_section` with/without the flag; `list_trash` on a
sidecar-less dir), integration (write `smb.conf` to a temp root and assert the
fragment; restore path from a tree-preserved entry), Playwright (Shares → SMB
tab toggle; Explorer → Trash → restore). **Docs**: `docs/guides/shares.md`,
`docs/guides/explorer.md`. **Effort**: S–M. **Depends on**: QW-02 (retention).

### QW-02 — Trash retention and size cap (auto-purge)

> **✅ Shipped 2026-10-29.** `browser::TrashPolicy { retention_days, max_bytes }`,
> default **30 days / 10 GiB** (either `0` = that limit off), stored in a new
> singleton `trash_policy` table (`src/storage/trash_policy_repo.rs`, migration
> `061_trash.sql`) with a `last_purge_at` stamp. The selection rule is pure and
> unit-tested (`FileBrowser::select_purge`: age first, then oldest-first to fit
> the size cap); `FileBrowser::purge_trash` applies it per root and reports
> `{ purged, freed_bytes }`. A background scheduler
> (`src/trash_scheduler.rs`) mirrors `journal_cleanup_scheduler` — an hourly
> poll that runs at most daily and only advances `last_purge_at` on a successful
> pass. An admin can run the pass on demand. APIs:
> `GET|PUT /explorer/trash/policy`, `POST /explorer/trash/purge-now`; the
> Explorer Trash view shows an admin-only retention panel with a "Clean up now"
> button. *Deviation from the proposal below:* not a `scheduled_tasks` kind — a
> dedicated scheduler loop, matching the other app-local schedulers.

**DSM reference**: recycle-bin retention and size management.

**Delivered behavior**: a configurable 30-day / 10 GiB default policy with a
background purge scheduler and an administrator-triggered cleanup action.
See `src/trash.rs`, `src/trash_scheduler.rs`, migration `061_trash.sql` and
`docs/guides/explorer.md`.

### QW-03 — Server-side reflink / instant copy in the Explorer

> **✅ Shipped 2026-10-29.** `src/browser/reflink.rs` gives every Explorer copy a
> three-step per-file cascade — `ioctl(FICLONE)` → `copy_file_range` → the
> historical verified SHA-256 stream — and reports which step ran
> (`CopyMethod::{Clone, KernelClone, KernelCopy, Stream}`; `KernelClone` is
> reserved, because no syscall can prove a kernel-side copy shared extents).
> The atomic temp-file + `rename` + metadata + parent-`fsync` contract is
> unchanged; a clone skips the read-back hash by construction, every fallback
> keeps it. Only `EXDEV`/`EOPNOTSUPP`/`ENOTTY`/`EINVAL`/`EBADF` mean "this pair
> cannot share extents" — `EIO`/`ENOSPC`/`EPERM`/`EROFS` are reported, so a
> failing disk is never hidden behind a slow copy that happens to succeed. The
> policy is the singleton `instant_copy_policy` row (migration
> `062_instant_copy.sql`), `InstantCopyMode::{Auto, Off}`, default `auto`,
> served by `GET|PUT /explorer/instant-copy` (read any signed-in, write admin —
> the request path parses **strictly**, so an unknown mode is refused rather
> than coerced) together with per-mount evidence from `/proc/self/mounts`
> (btrfs yes, XFS only with `reflink=1`, ZFS never claimed — the ioctl is the
> authority). Directory jobs tally every file in a lock-free `CopyStats`, and
> the activity row says "Copied N file(s) instantly (no data copied)" only when
> the **slowest** method of the tree is instant, "N cloned, M copied" for a
> mixed tree; a same-filesystem move reports `method: null`, since an atomic
> rename copied nothing. *Deviation from the proposal below:*
> `cp --reflink=auto` was rejected — it silently falls back to a plain copy and
> cannot prove it cloned, so the ioctl is issued in-process instead.
> **Verified live 2026-10-30** (scratch server `:8099`, `e2eadmin`): E2E §17
> **5/5 green** on Firefox. That run fixed a real design-system leak it
> exposed: `base.css`'s `input { width:100%; … }` also matched
> `<input type="radio">`, so the policy modal stretched to 1246 px in a 720 px
> viewport and its option text wrapped one word per line; the radios now carry
> `h-4 w-4 flex-none` and the mounts list is bounded (`max-h-64
> overflow-y-auto`), putting the dialog back at 580 px. It also fixed the
> spec's `openWork` helper, which `.first()`-matched a *stale* `e2ework`
> favorite from an earlier run (favorites are per-path and were never swept),
> so a filtered `-g "17 …"` run no longer poisons the next one. The first
> full-file run was 69/71; both remaining reds were then fixed, and the file is
> now **71/71** — see "Reliability fixes delivered" (RF-01/02/03).

**DSM reference**: `SYNO.Core.FileServ.ReflinkCopy` — File Station can share
extents for copies on compatible filesystems.

**Delivered behavior**: `/explorer/copy` answers `method`/`instant`/`bytes` for
a single file and a tracked job for a directory, whose completion message names
the clone result; `/explorer/move` answers `method: null` for an atomic rename.
See `src/browser/reflink.rs`, `src/browser.rs` (`copy_file_instant`,
`copy_dir_inner`, `move_item_with_mode`), `src/handlers/explorer.rs`,
migration `062_instant_copy.sql` and `docs/guides/explorer.md`.

### QW-04 — Share-link password, download cap and one-time links

> **✅ Shipped 2026-10-31**, tests green: unit 11, integration 21, E2E §16 5/5 live on Firefox + Chromium. Delivered scope and deviations: *Delivered* below.

**DSM reference**: `SYNO.Core.Sharing` / the `FileSharing` / `SharingManager`
modules — shared links support a password, an expiry, and download accounting.

**HomeVault today**: share links already have a high-entropy token, `anyone`
vs `signed_in` mode, expiry (default 30 days, or never), a per-token leaky-bucket
bandwidth cap and a QR code (`src/handlers/share_links.rs`). Missing: a
password, a maximum number of downloads, and a one-time option.

**Why it wins**: "send this to my neighbour, once" is the whole point of the
feature, and today the only knob is time. Three small additions close the gap.

**Proposal**: add `password_hash` (Argon2, same primitive as user passwords),
`max_downloads` + `download_count` and `one_time` to `ShareLink`
(`src/storage/share_link_repo.rs`); a `060x` migration; a password prompt on the
public `/share/{token}` page (a short-lived, single-use unlock cookie); an
increment-and-cut when the counter is reached.

**Acceptance**: a password-protected link refuses a wrong password and never
puts the password in a URL; the N-th download is the last; a one-time link is
dead after the first completed download; `GET` list responses never leak the
hash.

**Tests**: unit (counter/last-download arithmetic, hash verification), integration
(prompt → download → cap), Playwright (create form → open public page). **Docs**:
`docs/guides/explorer.md`. **Effort**: S–M.

**Delivered 2026-10-31** (`063_share_link_controls.sql`, `src/handlers/share_links.rs`,
`src/storage/share_link_repo.rs`, `frontend/src/routes/PublicShare.svelte`,
`ShareLinkModal.svelte`). The shared URL stays a one-click download: a browser
navigation that *cannot* be answered with the file (`Accept: text/html`, missing
password, spent/revoked link, signed-in-only link) receives the public page,
while a download manager or `fetch` keeps getting the file or the JSON status;
`signed_in` links accept the `hv_session` cookie so a tab works. Share
responses are `Cache-Control: no-store` and `sw.js` skips `/share/`.
`password_hash` (Argon2id) and
`max_downloads`/`download_count` are stored, never serialised; there is **no**
`one_time` column — `max_downloads = 1` *is* the one-time link, because two
mechanisms with separate arithmetic can disagree (`is_exhausted` is the only
place that decides). A slot is reserved atomically before the first byte and
`download_count` only rises when the transfer wrote its last byte, so nothing
failed/abandoned ever spends the link; the reservation is released on every
early return, on a read error and in the stream's `Drop` — see the defect list
below. The public page `HEAD`s the link first (spends no slot, and a spent
link answers `400` instead of failing after the password was typed), asks for a
password **only** when the link has one, and exchanges it at
`POST /share/{token}/unlock` for a 15-minute, path-scoped, `HttpOnly` cookie, so
the password never reaches a URL and the download is a native navigation
(ranges/resume, no whole file in JS memory). Wrong passwords: 8 per link per
5 minutes, refused *before* Argon2 runs; an empty password is a free probe.
`x-hv-share-auth: password|session` tells the page which factor is missing, and
token path segments outside `[A-Za-z0-9_-]{1,128}` are refused before being
echoed into a header.

Five defects of the in-flight implementation were fixed on the way: the `HEAD`
probe reported a spent link as downloadable; a failure between reserving a slot
and opening the file leaked the slot; a mid-stream disconnect leaked it too; a
**completed** download was recorded as interrupted (hyper stops polling a sized
body once `Content-Length` is written, so the end-of-stream poll never fires —
completion now follows the byte count, evaluated in `Drop` as well); and the
**service worker cached `/share/` cache-first**, letting a spent or revoked link
replay from the browser cache.

Tests — **all run, green** (not merely written): `handlers::share_links` unit **2 → 11**;
`share_links_integration.rs` **13 → 21** (incl. a test that reproduces hyper's
framing); E2E `explorer.spec.ts` §16.1–16.5 **5/5 live on Firefox and Chromium**
(2026-10-31, scratch server `:8099`, `e2eadmin` seeded via
`POST /api/auth/setup`); `cargo test --lib` **1874/1874**.

*Deviation from the proposal*: the unlock cookie is short-lived rather than
single-use — a video seek re-requests with `Range`, and a single-use cookie
would break resume mid-download.

### QW-05 — Login auto-block (persistent ban) and DoS settings

> **✅ Implemented 2026-10-31** — compile-checked; unit 16 (+2 in `firewall.rs`) / integration 14 / E2E 14 present. The integration suite is covered by the backend verification pass; the dedicated E2E suite has not been run to completion. Delivered scope and deviations: *Delivered* below.

**DSM reference**: `SYNO.Core.Security.AutoBlock` (rules + a block list),
`SYNO.Core.Security.DoS`, `SYNO.Core.SmartBlock` (trusted/untrusted
devices/users with *pardon*). Control Panel → Security → Account →
Auto Block.

**HomeVault today**: a per-account lockout after 5 failures (`src/auth/service.rs:79`
`MAX_LOGIN_ATTEMPTS`), login history + reusable lockout counting
(`src/storage/login_attempt_repo.rs`), and an **in-memory** per-IP limiter
(`src/middleware/rate_limit.rs`) that forgets a blocked attacker on restart. The
firewall is `ufw` (`src/firewall.rs`) but is never driven by login failures.

**Why it wins**: a home box exposed through QW-01… is scanned continuously; the
missing piece is that a block must *survive a restart* and reach the firewall.
It also finally gives the Security screen something a novice can act on.

**Proposal**

1. Persist a `bans` row (IP, reason, created, expires) when the failure window
   trips; enforce it in the auth middleware with a clear "temporarily blocked"
   response.
2. Optionally add the IP to `ufw` (reuse the existing rule writer) with an
   expiry and a one-click **Pardon**; never block the admin's current IP.
3. A Security card: enable/disable, threshold, window, ban duration, the current
   block list with pardon, and a DoS row (request-rate cap on the public
   listener).

**Acceptance**: N failures from one IP blocks it; the block survives a restart
and, when enabled, appears as a firewall rule; a pardon removes both
immediately; a blocked attacker cannot lock out a legitimate user from a
different IP.

**Tests**: unit (window → ban, expiry), integration (blocked login, pardon,
firewall rule generation against a mock), Playwright (Security card). **Docs**:
`docs/guides/security.md`, `docs/guides/firewall.md`. **Effort**: M.

**Delivered 2026-10-31** — implemented and compile-checked; **the test suites are
present; backend verification is partial and the dedicated E2E suite remains pending (see `TESTING.md`, 2026-10-31 QW-05).
(`src/auto_block.rs`, `src/handlers/auto_block.rs`,
`src/storage/ip_ban_repo.rs`, `src/middleware/auto_block.rs`, migration
`065_ip_bans.sql` — tables `ip_bans` + `auto_block_settings`,
`frontend/src/lib/components/settings/AutoBlockCard.svelte`).
`GET|PUT /security/auto-block`, `POST /security/auto-block/{pardon,clear,sweep}`,
all admin-only. The block is a row (restart-proof), checked before the password
is verified in `handle_login` **and** by `middleware::auto_block` for
unauthenticated `/api` calls only — a session granted before the ban is not
revoked by this layer, and an authenticated admin is never locked out by it.
`must_not_ban` refuses loopback, unspecified, non-addresses and the caller's own
address. With `apply_firewall` on, `src/firewall.rs::{ban_source,unban_source}`
install and remove `ufw deny from <ip>` tagged `homevault-autoblock-<ip>`
(both subcommands were already in the sudoers policy, so enabling the feature
grants nothing new); the ban row records whether the rule really was installed
and a pardon trusts that flag. Expiry is swept on the read path, so an old ban
lifts itself with no scheduler. The second row caps **unauthenticated** requests
per minute per address (`middleware::rate_limit::RateLimitState`, rebuilt when the
rate changes) — signed-in traffic is deliberately not counted. Tests: 16 unit +
2 in `firewall.rs`, 14 integration (admin matrix, defaults, round-trip, six
invalid policies, repeated failures banning, **ban surviving a restart**, **never
locking out another address**, loopback/caller never banned, pardon, clear,
**expired ban lifting itself**, ban surviving a failed firewall rule, DoS
round-trip), 14 E2E — *all written, none run yet*. Docs:
`docs/guides/security.md`, `docs/guides/firewall.md`, `docs/API.md`,
`SCREENS.md`. **Deviation from the proposal**: the DoS cap counts
*unauthenticated* traffic only (a shared NAT would otherwise throttle the
appliance's own polling), and enforcement is scoped to the login path plus
unauthenticated `/api` calls rather than every request.

### QW-06 — Connected clients list and kick

> **✅ Implemented 2026-10-31** — compile-checked; unit 16 / integration 6 / E2E 9 present. The dedicated integration suite passes; the E2E suite has not been run to completion. Delivered scope and deviations: *Delivered* below.

**DSM reference**: `SYNO.Core.CurrentConnection` — `list`, `list_by_user`,
`kick_connection`, `disable_user`; a live view of who is connected over which
service, not just the web UI.

**HomeVault today**: "Active Sessions" exists but covers only web sessions
(`src/handlers/security.rs:108` `handle_get_my_sessions`), i.e. the browser.
SMB/NFS/FTP/WebDAV/SFTP clients are invisible.

**Why it wins**: "why is the disk busy / who is copying" is a weekly question,
and the answer is free to collect on a Linux host.

**Proposal**: a read-only collector that combines `smbstatus -j`, `showmount -a`,
`ss -tnp` (filtered to the managed services) and the SFTP `sshd` connections
into one list (user, protocol, client IP, opened file), refreshed on demand;
`kick` maps to `smbcontrol <pid> close-share` / dropping the connection. Read
first, kick behind an admin confirmation that names the client.

**Acceptance**: a file held open over SMB appears with its client and path; the
list is empty on a quiet box; kicking a client frees the handle; non-admins see
only their own connections.

**Tests**: unit (parsers for `smbstatus -j`, `showmount -a`, `ss`), integration
(mocked command output), Playwright (Security → Connected clients). **Docs**:
`docs/guides/security.md`. **Effort**: M.

**Delivered 2026-10-31** — implemented and compile-checked; **the test suites are
present; the integration suite passes and the dedicated E2E suite remains pending (see `TESTING.md`, 2026-10-31 QW-06).
(`src/connected_clients.rs`, `src/handlers/connected_clients.rs`,
`frontend/src/lib/components/settings/ConnectedClientsCard.svelte`).
`GET /security/connected-clients` + `POST /security/connected-clients/kick`,
admin-only. Three independent sources — `smbstatus -j` (sessions with their
shares from `tcons` and an open-file **count** from `open_files`), `showmount -a`
(NFS mounts) and `ss -H -tnp state established` filtered to 445/139/22/21 —
each bounded at 5 s on the blocking pool, each reporting its own availability and
error, so **one missing tool never fails the list**. Rows carry the account, the
address/host, the share or export, the open-file count and the process evidence,
and **never an absolute path** (the card answers "who", not "what are they
reading"; a unit test asserts it). A kick names a **client id, never a pid**: the
collection is re-run and the pid comes from the fresh observation, so a stale or
forged id cannot signal an unrelated process. SMB uses
`smbcontrol <pid> close-share <share>` (the session survives, the user can
reconnect), SFTP/FTP a `TERM` to that connection's process (the server's own pid
is refused), and NFS is marked **not kickable** with the reason and the
alternative. Tests: 16 unit, 6 integration (admin matrix, **always-200 contract
with three sources**, no path leak, the three refusals), 9 E2E (mocked list +
kick, two-click confirmation) — *all written, none run yet*. Docs:
`docs/guides/security.md`, `docs/API.md`, `SCREENS.md`. **Deviation from the
proposal**: the list is a Security **card** rather than a new screen, reads
`ss` for the session processes (which is what makes a kick possible at all), and
NFS rows are explicitly non-kickable instead of silently omitting the action.

### QW-07 — Shared-folder quotas

> **✅ Implemented 2026-10-31** — compile-checked; unit 22 / integration 5 / E2E 9 present. Backend integration coverage exists, but the full dedicated E2E suite has not been run to completion. Delivered scope and deviations: *Delivered* below.

**DSM reference**: `SYNO.Core.Quota` — a size limit on a shared folder,
independent of per-user quotas.

**HomeVault today**: per-user home quotas exist and are well-built
(`src/quota.rs`: `setquota`/`repquota`/`zfs userquota`, Btrfs explicitly out of
scope; `docs/guides/quotas.md`), and transfer quotas cap bytes in/out
(`src/transfer_quota.rs`). A **share** has no size ceiling — one runaway folder
fills the pool.

**Why it wins**: complements a shipped feature instead of inventing a new
concept; the enforcement primitives are already abstracted behind
`QuotaBackend`.

**Proposal**: a per-share quota stored on the share, enforced with Btrfs qgroups
(`btrfs qgroup limit`) / ZFS `refquota` / ext4-XFS project quotas, shown as a
bar in Shares and on the Dashboard. Refuse uploads/copies that would cross it
(next to the existing transfer-quota checks in the Explorer).

**Acceptance**: with a 10 GB share quota, an 11 GB upload is refused with the
reason; usage is read back from the filesystem, not guessed; clearing the quota
restores unrestricted use; unsupported filesystems hide the field.

**Tests**: unit (per-filesystem command selection + usage parsing), integration
(loop-backed Btrfs/ZFS, skipped when unavailable), Playwright (Shares → quota
bar). **Docs**: `docs/guides/quotas.md`, `docs/guides/shares.md`. **Effort**: M.

**Delivered 2026-10-31** — implemented and compile-checked; **the test suites are
present; full dedicated E2E verification remains pending (see `TESTING.md`, 2026-10-31 QW-07).
(`src/share_quota.rs`, `src/handlers/share_quota.rs`,
`src/storage/share_quota_repo.rs`, migration `066_share_quotas.sql` — table
`share_quotas`, `frontend/src/routes/Shares.svelte`). The share has no row of its
own (it is derived from `smb.conf` / `/etc/exports`), so the **intent** lives in
`share_quotas` and the **enforcement** in the filesystem: ZFS `refquota`, a Btrfs
**subvolume's** qgroup, or an XFS project quota on a `prjquota` mount; everything
else (ext4, tmpfs, network mounts) answers `supported: false` with the reason and
the card hides the field — including the honest refusal of a share that is a
plain folder inside a Btrfs subvolume, where a qgroup limit would cap every share
in it. `GET|PUT|DELETE /shares/{name}/{share_type}/quota`, admin-only; the
filesystem is changed **first** and the row only records what it accepted, so
the card can never claim a limit that is not in force. Usage is read back
(`zfs list -p`, `btrfs qgroup show --raw`, `xfs_quota report -p -b -N`), never
guessed. The Explorer refuses an upload (up front, then per file — a file that
crosses the ceiling is deleted) and a copy (source size, real walk for a tree)
with `507` naming used/quota/incoming; protocol clients are bound by the
filesystem limit itself, so the two are complementary. Tests: 22 unit (backend
matrix incl. the two refusals, every argv, all parsers, the decision, **the
acceptance criterion verbatim**, validation, formatting), 5 integration (admin
matrix, unknown share/protocol, validation with nothing stored, the DB
round-trip, the payload's honest unsupported answer), 9 E2E (bar + progressbar
semantics, unsupported explanation, GB→bytes save, refusal as `alert`) — *all
written, none run yet*. Docs: `docs/guides/quotas.md`, `docs/guides/shares.md`,
`docs/API.md`, `SCREENS.md`, `docs/INDEX.md`. **Deviation from the proposal**:
ext4 project quotas are **not** wired (the tooling differs from XFS and would
have been a second untested path), so ext4 reports unsupported; and the
integration tests do not loop-mount Btrfs/ZFS (that needs root) — the backend
selection, argv and parsers are unit-tested instead, with the live half left as
a manual check.

### QW-08 — Remote syslog client

> **✅ Implemented 2026-10-31** — compile-checked; unit 23 / integration 11 / E2E 16 present. Backend integration coverage exists, but the full dedicated E2E suite has not been run to completion. Delivered scope and deviations: *Delivered* below.

**DSM reference**: `SYNO.Core.SyslogClient.*` (Log Center → Log Sending), RFC
5424/3164, TCP/UDP/TLS, per-category filters. [Also `ROADMAP-DSM.md` DSM-09.]

**HomeVault today**: the Journal reads local sources
(`src/handlers/journal.rs`); nothing is forwarded off-box.

**Why it wins**: `S` effort, and it is the one audit feature a small office or a
security-minded home user actually asks for.

**Proposal**: `src/syslog_forward.rs` — destination, protocol, format, facility,
optional TLS with CA pinning, a bounded in-memory queue drained by a background
task, and forwarding of HomeVault's own audit events beside the system sources.
Never block a request on a syslog write; reuse the existing redaction path.

**Acceptance**: a `logger` event reaches a test collector in the chosen format;
an unreachable collector drops oldest-first and reports "degraded"; secrets are
not forwarded.

**Tests**: unit (RFC 5424 framing, queue bounds), integration (local UDP/TCP
collector), Playwright (Settings → Monitoring → Syslog). **Docs**:
`docs/guides/journal.md`. **Effort**: S.

**Delivered 2026-10-31** — implemented and compile-checked; **the test suites
are present; full dedicated E2E verification remains pending (see `TESTING.md`, 2026-10-31 QW-08).
(`src/syslog_forward.rs`, `src/handlers/syslog.rs`,
`frontend/src/lib/components/settings/SyslogCard.svelte`, settings in
`data/syslog_forward.json` — JSON storage like DDNS/TLS, so no migration).
`GET|PUT /settings/syslog` + `POST /settings/syslog/test`, all admin-only.
`events::log_event`/`log_event_warn` hand their sentence to a bounded
drop-oldest queue (1024) drained by one worker in batches of 64, one connection
per batch, 5 s deadlines on connect and write — a log call never touches the
network, and the `enabled` flag is cached for a second so a disabled feature
costs a log line nothing. Framing is pure and asserted byte for byte (PRI
arithmetic, `-` nil values, space-in-header substitution, RFC 3164 day padding,
UDP truncation on a character boundary). TLS is `tokio-rustls` with a client
trusting **only** the pinned CA (system roots are not loaded); the crypto
provider is named (`ring`) because the tree carries two. The test button says
"sent — check the collector" rather than claiming delivery. Tests: 23 unit,
11 integration (admin matrix, validation, **real localhost UDP and TCP
collectors**, unreachable collector reported without failing the save, audit
event queued only while enabled, an observation write that cannot revert a
newer configuration), 16 E2E — *all written, none run yet*. Docs:
`docs/guides/journal.md`, `docs/API.md`, `SCREENS.md`. **Deviation from the
proposal**: the proposal listed "forwarding of HomeVault's own audit events
beside the system sources"; only the former ships — re-shipping `/var/log`
would duplicate every line on the collector and cannot be bounded by this queue.

### QW-09 — Wake-on-LAN

> **✅ Implemented 2026-10-31** — compile-checked; unit 17 / integration 7 / E2E 13 present. Backend integration coverage exists, but the full dedicated E2E suite has not been run to completion. Delivered scope and deviations: *Delivered* below.

**DSM reference**: `SYNO.Core.Hardware` WOL; Control Panel → Hardware & Power.
[Also `ROADMAP-DSM.md` DSM-08.]

**HomeVault today**: `src/power.rs` reboots/shuts down/suspends and does disk
spindown/APM (`DiskSpindownSetting`, `hdparm_args`), but cannot wake a host.

**Why it wins**: HomeVault is often installed on a machine that sleeps; "wake
the NAS, then run the 03:00 backup" is the natural pairing and is a magic packet
plus an interface pick.

**Proposal**: a `wol_hosts` table (label, MAC), send a magic packet on UDP 9
and 7, broadcast on the interface that owns the target subnet; a "Wake first"
option on scheduled tasks; "sent, not confirmed" wording on success.

**Acceptance**: a powered-off host on the same L2 segment wakes; the packet
leaves the right interface; failures say "sent, not confirmed".

**Tests**: unit (packet bytes, interface selection), integration (UDP listener
captures the packet), Playwright (Settings → Power). **Docs**:
`docs/guides/power.md`. **Effort**: S.

**Delivered 2026-10-31** — implemented and compile-checked; **the test suites
are present; full dedicated E2E verification remains pending (see `TESTING.md`, 2026-10-31 QW-09).
(`src/wol.rs`, `src/handlers/wol.rs`, `src/storage/wol_host_repo.rs`,
`wol_hosts` = migration `064_wol_hosts.sql`,
`frontend/src/lib/components/settings/WolCard.svelte`).
`GET|POST /power/wol/hosts`, `PUT|DELETE /power/wol/hosts/{id}` and
`POST /power/wol/wake`, all admin-only. The packet is the standard
6 × `0xFF` + the MAC sixteen times (102 bytes) sent to ports 9 **and** 7;
the destination is the machine's explicit override, else the **directed
broadcast** of the selected interface (chosen by target subnet from the
`/sys/class/net` inventory, never a subprocess), else `255.255.255.255`. The
answer is `sent`, never `woken`, and the wording is asserted in a unit test.
MAC parsing accepts the four spellings an admin pastes and refuses a wrong
length, non-hex, all-zero and any multicast/broadcast address. "Wake first" is
a `wake: { host_id, delay_seconds }` pre-step on **any** task type (not a fifth
type), validated at save time and again before the run, and a host that is gone
or unreachable **fails the run** rather than letting the task hit a sleeping
box. Tests: 17 unit, 7 integration (admin matrix, CRUD, ten invalid hosts, a
real localhost UDP socket receiving the 102 bytes, the wake report's wording,
the task gate), 13 E2E — *all written, none run yet* (compile-checked:
`cargo check --all-targets`, `npm run check` 0/0, `npm run build` OK). Docs:
`docs/guides/power.md`, `docs/guides/scheduled-tasks.md`, `docs/API.md`,
`SCREENS.md`. **Deviation from the proposal**: the interfaces are read from
`/sys/class/net` (the Network screen's existing inventory) instead of shelling
out to `ip`, so the card and a wake never fork; and the saved list is a
`wol_hosts` table as proposed (ids are what a task payload references).

### QW-10 — Security Advisor checklist

> **✅ Shipped 2026-10-31.** `src/security_advisor.rs` evaluates 19 findings from
> an `AdvisorFacts` value (pure, so both directions of every check are
> unit-tested) and `collect()` gathers them from the services the other screens
> already read; `GET /api/security/advisor` (admin-only) answers the report and
> there is **no write route** — every fix is a deep link. The card lives at the
> top of the Health screen for administrators. Tests: **27 unit + 10
> integration** (+ `tests/security_advisor_integration.rs` pins the "fix it and
> the finding is gone on the next call" criterion and a payload with no
> secret), one i18n parity test over the four locales, **+ 10 E2E** in
> `tests/e2e/health/health.spec.ts` §13; the whole Health file is **47/47 green
> on Firefox** against a rebuilt binary on a scratch server. *Deviations from the proposal below:* findings are English
> sentences computed server-side rather than i18n keys (the report is also an
> API/MCP surface, where a browser locale is meaningless — the card's own
> chrome is fully translated ×4), and there is no persisted score table (a
> stored score can disagree with the live reading; the dashboard tile can read
> the endpoint).

**DSM reference**: `SYNO.SecurityAdvisor` + the `SecurityScan` module — pass /
warn / fail with a one-click fix. [Also `ROADMAP-DSM.md` DSM-18.]

**HomeVault today**: Health, Firewall, Antivirus, Anti-Ransomware and TLS each
show their own state; nothing aggregates "is this box safely configured?" for a
novice.

**Why it wins**: the highest raw value on this list (`5`) because it turns a
dozen technical cards into one answer. The checks read state HomeVault already
computes, so the work is aggregation, not new plumbing.

**Proposal**: a read-only audit engine (`src/security_advisor.rs`) over existing
signals — admin present and not named `admin`; 2FA on admins; TLS valid and not
expiring (reuse ACME status); firewall on with the expected ports; SMB signing;
guest access off; snapshots on data pools; a backup configured and last run
green; updates applied; no share world-writable. Each finding carries severity,
evidence and a deep link; a per-run score feeds the Dashboard.

**Acceptance**: a fresh install scores exactly what the fixture says; each check
has a positive and a negative integration case; fixing a finding clears it on
the next run without a restart.

**Tests**: unit (every check, both ways), integration (endpoint + auth),
Playwright (Health → Advisor). **Docs**: `docs/guides/health.md`. **Effort**: M.

### QW-11 — Full configuration backup and restore

> **✅ Implemented 2026-10-31** — compile-checked; unit 13 / integration 6 / E2E 11 present. Backend integration coverage exists, but the full dedicated E2E suite has not been run to completion. Delivered scope and deviations: *Delivered* below.

**DSM reference**: `SYNO.Core.ConfigBackup` / the `ConfigBackup` module —
export settings, users and shares, optionally encrypted, restore on a new box.
[Also `ROADMAP-DSM.md` DSM-14.]

**HomeVault today**: a JSON settings export/import exists
(`src/handlers/settings.rs:1831` `handle_settings_export`, `:1868` import,
backed by `src/settings_persistence.rs`), and the DB is dumped before a
migration (`src/db_backup.rs`). It does **not** carry users, groups, shares,
ACLs or generated configs, so it cannot rebuild a box.

**Why it wins**: "move HomeVault to new hardware" is a guaranteed lifecycle
event; the export/import UI and the settings-persistence layer already exist, so
this is an extension, not a new subsystem.

**Proposal**: a signed, optionally passphrase-encrypted archive containing a
consistent DB snapshot (`VACUUM INTO`), the generated configs (`smb.conf`,
`/etc/exports`, netplan, the SFTP `sshd`, ufw rules) and a version manifest.
Secrets are excluded by default and included only behind an explicit
"include secrets" + passphrase; import refuses a newer schema.

**Acceptance**: export → wipe instance → import restores users, groups, shares
and settings; a wrong passphrase fails cleanly; a newer schema is refused with a
clear message; by default the archive contains no plaintext secret.

**Tests**: unit (manifest, redaction, schema check), integration (round-trip on
a temp data dir), Playwright (Settings → System). **Docs**:
`docs/guides/settings.md`, `docs/guides/backup-restic.md`. **Effort**: M.

**Delivered 2026-10-31** — implemented and compile-checked; **the test suites are
present; full dedicated E2E verification remains pending (see `TESTING.md`, 2026-10-31 QW-11).
(`src/config_backup.rs`, `src/handlers/config_backup.rs`,
`frontend/src/lib/components/settings/ConfigBackupCard.svelte`). One container
(`HVCONFIG1` + a JSON manifest + concatenated entries, no tar/gzip dependency)
holding a `VACUUM INTO` database snapshot, the settings export and an
**allow-listed** set of generated configs (`smb.conf`, `/etc/exports`, netplan,
the SFTP `sshd`, the ufw rules); every entry carries a SHA-256, and a file that
was missing at export time is recorded with its reason in `manifest.skipped`.
Optional encryption is AES-256-GCM under an Argon2id-derived key, and the
encoder itself refuses a secrets archive without a passphrase.
`GET /settings/config-backup`, `POST …/export`, `GET …/archives/{id}`,
`POST …/inspect`, `POST …/restore`, admin-only; the archive id is allow-listed
**and** path-confined, the passphrase travels in a header, and the download is
`no-store`. Restore is **staged** (`data/restore/`, `READY` written last) and
applied by `apply_pending_restore()` at startup, **before** the database is
opened, keeping `homevault.db.pre-restore`. Tests: 13 unit (round-trip both
encrypted and not, the payload unreadable when encrypted, four refusal
categories, a tampered entry named, truncation, the allow-list/scrub-list
guards), 6 integration (admin matrix, the full export → inspect → restore →
apply round-trip, the secrets/passphrase paths, **the no-plaintext-credential
acceptance criterion**, a newer container refused with nothing staged, three
crafted archive ids refused), 11 E2E (mocked endpoints, two-click restore,
pending-restore banner) — *all written, none run yet*. Docs:
`docs/guides/settings.md`, `docs/API.md`, `SCREENS.md`, `docs/INDEX.md`.
**Deviation from the proposal**: the archive is **not signed** (no signature
infrastructure exists in this build) — integrity comes from per-entry SHA-256
plus the AEAD tag when encrypted, and the docs say so; the restore **stages for
the next start** rather than swapping a live SQLite file; and the generated
configs are staged rather than written to `/etc` from a request (they need root,
and the answer lists them in `needs_root`).

### QW-12 — Universal file search across shares

> **✅ Shipped 2026-10-31.** `src/global_search.rs` (pure validation, root
> dedupe, merge/rank/dedupe, path confinement) + `handlers/search.rs`
> (`GET /api/search/files`, admin *and* non-admin, fanned out over the Explorer
> sidebar's own roots via the new `explorer::explorer_roots_for_session`), and
> the overlay half in `frontend/src/lib/stores/fileProvider.ts` +
> `explorerIntent.ts` + `SearchBar.svelte`/`SearchResults.svelte`. Tests: **17
> unit** (`src/global_search.rs`), **13 integration**
> (`tests/global_search_integration.rs`, two real roots on a temp dir), **6 E2E**
> (`tests/e2e/desktop/desktop.spec.ts` §6b), plus one i18n parity test.
> *Deviations from the proposal below:* `files_only` was added (the overlay
> needs it to keep file results tight), the fan-out is capped at 16 roots with a
> 1.5 s per-root and 4 s total budget, and the answer reports `timed_out` /
> `unreadable` / `truncated` so a partial result can never masquerade as a
> complete one. Content indexing remains out of scope, as proposed.

**DSM reference**: `SynologyUniversalSearch` package + `SYNO.Core.UISearch` —
search file names and indexed content across every shared folder, with filters.

**HomeVault today**: Ctrl+K searches apps and settings
(`frontend/src/lib/components/search/SearchBar.svelte`), and the Explorer's
search is scoped to a path you are already inside (`handle_explorer_search`,
`src/handlers/explorer.rs:908`, recursive/fuzzy under that root). There is no
"find this file anywhere I can read".

**Why it wins**: the desktop already has the search surface; the missing piece
is a cross-root file provider with the same ACL filter the Explorer uses.

**Proposal**: add a file provider to the desktop search overlay: fan out the
existing recursive search over every allowed root the session can read
(`state.path_guard`), merge, dedupe, rank, cap results; respect the same
`validate_path_access` filter. Content indexing is explicitly out of scope for
this win (name search only).

**Acceptance**: a filename only present in a share the user can read is found
from the desktop, and a file in a share the user *cannot* read never appears;
results are capped and the search is bounded in time; an empty query is a no-op.

**Tests**: unit (merge/rank/dedupe, ACL filter), integration (endpoint over two
roots, one denied), Playwright (Ctrl+K → result → open in Explorer). **Docs**:
`docs/guides/explorer.md`, `docs/guides/desktop.md`. **Effort**: M.

### QW-13 — Explorer transfer visibility (tray, detail, history, checksums)

> **✅ Implemented 2026-10-31** — `cargo check` clean; new unit + integration
> tests written, E2E section 18 written. The dedicated E2E run has **not** been
> executed yet. Delivered scope below.

**DSM reference**: FileStation's three-zone transfer tray
(`ui/task_monitor/TaskGrid.js` + `UploadMonitor.js`), its dedicated checksum
module (`webapi/SYNO.FileStation.MD5.so`) and its "completed tasks" list.

**HomeVault today (before this item)**: transfer *fundamentals* were ahead —
chunked resumable uploads (session 30 min), a SHA-256 `X-Content-Sha256` header
on downloads, an `X-Expected-Sha256` upload check, a reflink/FICLONE copy
cascade and per-subject transfer quotas with RAII refunds — but the *visibility*
layer was thin: one progress percentage (plus bytes and speed on the event, none
of it on the activity wire), a single generic taskbar entry per job, no
per-transfer cancel outside the desktop panel, no way to ask for a checksum, and
no record of a finished transfer anywhere. Upload sessions lived in one
in-memory `HashMap`, so a restart dropped every partial upload.

**Delivered** (details, API contract and invariants in `docs/guides/explorer.md`
→ "Transfer tray, enriched progress and history"):

| Part | What shipped |
|------|--------------|
| HV-UX-02 | `ProgressEvent` / `ActivitySnapshot` / `ActivityTask` carry `bytes_processed`, `bytes_total`, `speed_bps`, `eta_seconds` and `method`; `SpeedTracker` (rolling 5 s window) computes speed and ETA server-side; `frontend/src/lib/utils/transferFormat.ts` formats them (`45.3 MB / 128.7 MB · 12.0 MB/s · 2 min left`) |
| HV-UX-01 | `TransferTray.svelte` + `GET /api/explorer/transfers/active`: three zones (uploads / downloads / copies) with badges, a progress bar per row, and a per-row cancel that really stops the work (session abort for an upload, `DELETE /api/progress/{id}` for a job) — offered only when the row can be stopped (`has_cancel_hook`), so an upload and a checksum job show it and a directory copy does not |
| HV-UX-03 | Migration `074_upload_sessions.sql` + `src/upload_session.rs`: sessions are rows, rehydrated on a cache miss, so an upload resumes after a restart; the TTL is idle-based and the concurrency caps count rows |
| HV-INT-01 | `POST /api/explorer/checksum` + `GET /api/explorer/checksum/result`: SHA-256 / SHA-512 / BLAKE3, inline ≤ 8 MiB or as a tracked job, `compare_with` for "prove the copy is identical"; MD5 refused |
| HV-ADV-02 | Migration `073_transfer_history.sql` + `src/transfer_history.rs`: one row per finished upload/download/copy/move/checksum with bytes, duration, method, `verified` and status; `GET /api/explorer/transfers/history` (owner or admin), 90-day retention, rendered in the tray |

**Deviations**: no per-task pause/resume (pausing a server-side stream needs a
different data path than these synchronous copies and client-driven chunks;
cancel + resume-from-offset covers the user need). HV-ADV-01 (broadcast copy to
N destinations) was **not** taken: it is the only part of the source comparison
that adds a new job model rather than surfacing what already runs, and nothing
else in the appliance can consume it yet.

**Tests**: `checksum` 16 unit, `transfer_history` 11 unit, `upload_session` 5
unit, `sse::progress` +8 (tracker/ETA/method), `handlers::activity` +2,
`handlers::transfers` 5 unit, `explorer_integration` +16 (checksum inline /
algorithms / MD5 refusal / compare / async result / outside-root refusal /
no-session, history record / scoping / bad filter / paging, active transfers
auth / upload listing + cancel, upload resume from the database / orphan sweep /
cap counted in rows), `tests/e2e/explorer/explorer.spec.ts` section 18 (7 tests:
tray zones, upload listed + cancelled, history row, checksum + compare + MD5
refusal, cross-user refusal, keyboard/Escape, 360/768/1920 px).

---

## Quick-win sequencing

| Batch | Items | Theme | Why together | Status |
|---|---|---|---|---|
| **A — Recovery & trust** (≤ 1 week) | QW-01, QW-02, QW-03 | Make deletion recoverable and copies instant | Same files touched (`src/shares.rs`, `src/browser.rs`, `src/handlers/explorer.rs`); QW-02 is the safety valve QW-01 needs | ✅ shipped (2026-10-29) |
| **B — Sharing & access** (≤ 1 week) | QW-04, QW-08, QW-09 | Finish shipped features cheaply | All `S`/`S–M`, independent, no new subsystem | ✅ implemented 2026-10-31 — QW-04 **tests green** (unit 11, integration 21, E2E 5/5 live); QW-08/QW-09 **E2E-verified** 2026-11-02 (six-suite pass, 70/70 on Firefox and Chromium — see P0.1) |
| **C — Deterrence & visibility** (1–2 weeks) | QW-05, QW-06 | Survive a restart, see who is connected | Same Security screen and middleware | ✅ implemented 2026-10-31 — QW-05 (unit 16+2, integration 14, E2E 14) and QW-06 (unit 16, integration 6, E2E 9); QW-06 **E2E-verified** 2026-11-02 (six-suite pass, 70/70 on Firefox and Chromium — see P0.1) |
| **D — Capacity & rebuild** (1–2 weeks) | QW-07, QW-11 | Never fill the pool; rebuild the box | Both extend `src/quota.rs` / `settings_persistence` | ✅ implemented 2026-10-31 — QW-07 (unit 22, integration 5, E2E 9) and QW-11 (unit 13, integration 6, E2E 11); **E2E-verified** 2026-11-02 (six-suite pass — see P0.1) |
| **E — Guidance & discovery** (1–2 weeks) | QW-10, QW-12 | Tell the user what to fix and where things are | Both are aggregation over existing signals | ✅ both shipped 2026-10-31 — QW-10 (unit 27, integration 10, E2E 10/10 + 47/47 on the whole Health file, Firefox live) and QW-12 (unit 17, integration 13, E2E 6) |
| **F — Transfer visibility** (1–2 weeks) | QW-13 | Show, verify and remember what moved | All five parts hang off the same two files (`src/handlers/explorer.rs`, `src/sse/progress.rs`) and the same tray | ✅ implemented 2026-10-31 — unit 47 across the five modules, `explorer_integration` +15, E2E **section 18 (4 tests, 18.1–18.4)** written — **not yet executed** against a live server, so this row stays the one QW without an E2E count (the seven-test figure was an estimate; the file carries four) |

Dependency notes:

- **QW-01 → QW-02** (retention must exist before SMB deletion feeds the trash).
- **QW-05** wants the firewall writer used by `src/firewall.rs`; do it after the
  current firewall work settles.
- **QW-11** is the highest-value `M` if hardware is likely to change soon; it can
  be pulled forward without breaking the batches.

## Quick-win exclusions

The non-goals and deliberate divergences above apply to this shortlist too.
Additional items deliberately deferred here:

| Not doing here | Why |
|---|---|
| HDD hibernation curves beyond the shipped spindown/APM | `src/power.rs` already covers the useful cases; aggressive hibernation harms disk life |
| LED-brightness / beeper control | Cosmetic, hardware-specific, low value for the target user |
| DHCP / TFTP server | Real risk of breaking the LAN, low value (see DSM-21) |
| A media index / transcoding | The Docker catalog (Immich, Jellyfin, Navidrome) is the chosen path (DSM-16) |

## Traceability

| DSM evidence (this tree) | Item |
|---|---|
| `webapi/SYNO.Core.RecycleBin.lib`, `logrotate.d/synorecycle`, `syslog-ng/…/synorecycle.conf` | QW-01, QW-02 |
| `webapi/SYNO.Core.FileServ.ReflinkCopy.lib` | QW-03 |
| `webapi/SYNO.Core.Sharing.lib`, modules `FileSharing`, `SharingManager` | QW-04 |
| `webapi/SYNO.Core.Security.AutoBlock.lib`, `.DoS.lib`, `.SmartBlock.lib` | QW-05 |
| `webapi/SYNO.Core.CurrentConnection.lib` | QW-06 |
| `webapi/SYNO.Core.Quota.lib`, `SYNO.Snap.Usage.Share.lib` | QW-07 |
| `webapi/SYNO.Core.SyslogClient.lib`, module `LogCenter` | QW-08 |
| `webapi/SYNO.Core.Hardware.lib` | QW-09 |
| `webapi/SYNO.SecurityAdvisor.lib`, module `SecurityScan` | QW-10 |
| `webapi/SYNO.Core.ConfigBackup`, module `ConfigBackup` | QW-11 |
| package `SynologyUniversalSearch-*.spk`, `webapi/SYNO.Core.UISearch.lib` | QW-12 |
| `packages_extracted/FileStation-*/ui/task_monitor/`, `webapi/SYNO.FileStation.MD5.so`, `SYNO.FileStation.BackgroundTask.so` | QW-13 |

## Quick-win definition of done

Each item follows the Definition of done above: unit and integration Rust tests,
Playwright under `tests/e2e/<area>/`, an append-only migration when needed,
i18n parity, the matching guide and `FeatureDoc`, `docs/INDEX.md`, `SCREENS.md`
and `TESTING.md` updated. Every new root command must be narrowly scoped and
bounded; secrets must never reach logs, the journal or API responses.

Every edited screen must meet the accessibility baseline and simplicity rules
in `AGENTS.md` and `CLAUDE.md`.

## Static extraction follow-up — 2026-10-30

This section records a second pass over the supplied extracted firmware tree
`DSM_DS925+_90080/`. It is static evidence only: no DSM code was executed, and
no claim below implies the capability is enabled on a live DS925+ or available
without a license, provider account, compatible filesystem or Synology cloud.

### Inventory observations

| Evidence in supplied tree | What it establishes | Comparison note |
|---|---|---|
| `VERSION` | DSM 7.4.1, build 90080, `synology_v1000nk_ds925+` | Exact firmware/model identity for this comparison. |
| `hda1/usr/syno/synoman/webapi/SYNO.*.lib` | 127 API manifest files are present | Manifests declare API names/methods, not necessarily public support or runtime behavior. |
| `hda1/usr/syno/synoman/webman/modules/` | 58 UI module directories; notable modules include AdminCenter, LogCenter, ResourceMonitor, SecurityScan, StorageManager and ConfigBackup | UI resources corroborate user-facing workflows better than API names alone. |
| `packages_extracted/*/INFO` | 19 package manifests; includes FileStation, SMBService, StorageManager, ScsiTarget, SecureSignIn, QuickConnect, SynoFinder, USBCopy, DhcpServer, HighAvailability, HybridShare, HyperBackup-ED, BackupRestoreManager and ActiveInsight | Package presence is not proof of package installation, license, account entitlement or service reachability. |
| `AdminCenter/index.conf` | Shared-folder keywords/resources include recycle bin, quota, WORM/WriteOnce; modules/resources also identify DDNS, router configuration, traffic control, certificates, security and indexing | Strong evidence that these workflows are represented in this firmware UI. Backend enforcement still needs package/config evidence or runtime validation. |
| `ResourceMonitor/index.conf` | Resource/performance, connections, speed limits, settings and performance-alarm resources | Supports reviewing connection visibility, bandwidth controls and threshold alerts as a group. |
| `LogCenter/index.conf` and module assets | Log Center UI; archive, reports and settings assets exist | The first shortlist understated DSM log-management scope; remote syslog is only one part. |
| `packages_extracted/*/INFO` descriptions | QuickConnect uses Synology relay; Hybrid Share uses C2; Active Insight is a cloud service; BackupRestoreManager targets a remote Synology NAS | These features carry vendor-service, account and data-egress dependencies; do not treat as local/offline parity items. |

### Comparison corrections

| Existing roadmap claim | Static/source cross-check | Correct interpretation |
|---|---|---|
| DSM-06 says plain SMB only | HomeVault has SMB, but presence/absence of `vfs_fruit`, Time Machine options and `_adisk._tcp` advertisement was not conclusively established by this pass | Keep as **verify / likely gap**, not as a fully proven absence. Validate generated Samba config and mDNS behavior before implementation. |
| DSM-11 says local snapshots only | HomeVault documents scheduled ZFS/Btrfs snapshots with GFS retention, Restic backup/restore and pre-backup snapshots | Gap is off-box snapshot-stream replication/resume, not local snapshot scheduling. |
| DSM-14 says only pre-migration DB dump | HomeVault has user-facing settings JSON export/import and scheduled database snapshots | Gap is a complete, portable appliance migration bundle including identity/shares/protocol configuration, with explicit secret handling. |
| DSM-18 says Health and Firewall are the only related checks | HomeVault also ships Antivirus, ransomware monitoring, TLS/ACME, login history and security configuration | Security Advisor remains an **aggregation/usability gap**, not missing security signals. |
| DSM-19 fan control | HomeVault has temperature/SMART health telemetry; write-capable fan/PWM control was not established by this source review | Keep as hardware-dependent idea; never infer that DSM fan profiles map safely to generic Linux hwmon. |
| QW-12 universal file search | Existing HomeVault Explorer code supports searching beyond a single current directory; this pass did not establish a background full-text index | Reframe as optional **indexed content search**, subject to strict privacy, ACL, exclusion and retention design. |
| QW-06 client visibility | HomeVault has web session lists and desktop active-session counts; DSM exposes a broader current-connection workflow and Resource Monitor connection tab | The likely gap is per-protocol live connections, opened files and safe disconnect, not generic user-session visibility. |
| QW-07 share quotas | HomeVault has per-user home quotas and transfer quotas; DSM manifests/UI show quota and share-usage workflows | Share-level storage enforcement is still a candidate; first verify existing ZFS/Btrfs/project-quota support and distinguish storage quota from transfer quota. |

### Additional candidates from the firmware inventory

These supplement, rather than replace, the QW register above. QW-13 was taken
by the transfer-visibility item above; **IDs QW-14 onward are provisional** —
update the ranked table and traceability table before any item is accepted into
implementation.

| Candidate | DSM evidence | HomeVault comparison | Recommendation |
|---|---|---|---|
| Shared-folder WORM / WriteOnce | `AdminCenter/index.conf` has `WORM`, `WriteOnce` and `worm_share_edit.html` | Trash retention and snapshots are recoverability controls, not a lock against privileged deletion of live data | **Investigate, high value / high risk.** Establish actual filesystem guarantees and break-glass semantics before UI work. Never promise WORM on an unsupported backend. |
| Log archives and reports | LogCenter UI/resources include archive and report workflows; syslog client APIs are also present | HomeVault has local journal/event logs and cleanup, but no documented scheduled archive/report workflow or remote collector | **Consider as two features:** portable bounded archive/export first; optional TLS syslog forwarding second, disabled by default and using existing redaction. |
| Performance threshold alerts | ResourceMonitor help includes performance alarms; HomeVault has historical Metrics and notifications | Metrics/history exist; configurable threshold rules and alert hysteresis should be checked before declaring a gap | **Verify, then improve only if absent.** Reuse metrics and notification engine rather than adding a parallel monitor. |
| Full-device backup to a remote NAS | BackupRestoreManager description states configuration, packages and data can be sent to a remote Synology NAS; HyperBackup-ED is present | HomeVault has Restic and snapshots; no equivalent vendor-agent compatibility should be assumed | **Prioritize generic disaster-recovery proof:** test restore and documented rebuild. Consider restricted Restic receive mode; do not clone proprietary Synology protocols. |
| Hybrid cloud namespace | HybridShare package metadata describes C2 + on-premises caching | HomeVault has rclone sync and Syncthing, not a transparent cloud-backed namespace | **Defer.** High privacy/availability/complexity cost; only revisit with user-chosen providers, opt-in data egress and explicit cache/offline behavior. |
| Device discovery and relay | QuickConnect package and AdminCenter resources show relay and remote access configuration | HomeVault now has ACME, DDNS, reachability checks and VPN; no vendor-operated relay | **Do not copy QuickConnect.** Keep the self-hosted/VPN path; relay implies central infrastructure, account dependency and third-party traffic. |
| Online drive compatibility database | `SynoOnlinePack_v2` metadata says it updates a drive database online; StorageManager includes compatibility resources | HomeVault reports SMART/health and supports common disks, but no vendor-curated compatibility service is established | **Low priority.** A local, community-maintained compatibility list may help; never block non-listed drives without a concrete safety reason. |
| Vendor HA clustering | HighAvailability package metadata describes active/passive two-NAS takeover | HomeVault is a single-appliance target with snapshots/backup; clustering implies fencing, shared identity and split-brain handling | **Out of scope** for home appliance parity. Prefer tested restore and replication rather than claiming HA. |

### Corrected implementation priorities

1. **Verify before building:** Time Machine Samba/mDNS settings, share quotas,
   performance thresholds, cross-root filename search and protocol connection
   visibility against current source and guides. These were not all conclusively
   classified as absent in the first pass.
2. **High-value candidates:** full configuration migration/restore, Security
   Advisor aggregation, per-protocol connection visibility, and a proven
   disaster-recovery/test-restore flow.
3. **Evaluate with safeguards:** WORM/retention lock and content indexing. Both
   create strong user expectations and need explicit filesystem, ACL, privacy,
   retention and bypass semantics.
4. **Defer or reject:** QuickConnect relay, Synology C2 Hybrid Share, vendor HA,
   Synology-only backup compatibility and cloud telemetry unless the product
   explicitly adopts the service, account, data-egress and operational costs.

### Evidence limits

- Package `INFO` descriptions are vendor-authored summaries, not proof of every
  behavior, supported model entitlement or operational requirement.
- `SYNO.*.lib` files are API manifests. A method declaration does not prove the
  corresponding service is installed, licensed, enabled or reachable.
- UI `index.conf`, help paths and strings establish advertised workflow names;
  they do not prove the backend implementation is complete or that DS925+ hardware
  exposes the relevant device controls.
- HomeVault capability checks are static source/guide searches in the current
  worktree. “Not documented” is deliberately used where absence was not proven.
- A runtime DSM comparison would require a live, authorized DSM instance and
  explicit validation of packages, licenses, services and actual UI workflows.

## Reliability fixes delivered — 2026-10-30

Defects found by running the Explorer E2E suite against a live seeded server
while shipping QW-03. They are recorded here because each one was a *correctness*
problem rather than polish, and two of them were invisible to `cargo test`,
`svelte-check` and the unit suites.

### RF-01 — The policy modal outgrew the viewport (design-system leak, all screens)

`base.css` declares `input, select, textarea { width: 100%; border;
padding: … }` for *text* fields, and the selector also matches
`<input type="radio">` and `<input type="checkbox">`. A bare
`<input type="radio" class="flex-none">` therefore rendered 100% wide: it
stretched across its `<label>`, the option text wrapped one word per line, and
the QW-03 policy dialog measured **1246 px tall in a 720 px viewport**, pushing
its own controls out of reach (E2E 17.3 failed with "outside of the viewport").

- **Fix**: explicit box on the control — `h-4 w-4 flex-none
  accent-[var(--hv-primary)]` — the pattern `CloudSync.svelte` and
  `Rescue.svelte` already used; plus `max-h-64 overflow-y-auto` on the mounts
  list so a host with many filesystems scrolls the evidence instead of pushing
  the radios away. Dialog back to **580 px**, radio **16×16**.
- **Scope**: every screen that renders a bare radio/checkbox inherits the leak.
  Documented as a pitfall in `docs/guides/design-system.md` §7 (pitfall 6) and
  in `docs/guides/explorer.md`.
- **Not changed**: `base.css` itself. Narrowing the selector to
  `input:not([type="radio"]):not([type="checkbox"])` would be the structural
  fix, but it re-skins every form control in the product at once and belongs in
  a dedicated design-system pass, not inside a feature change.

### RF-02 — Admin delete was confined where admin create is not (asymmetry)

`handle_explorer_delete` was the **only** admin filesystem handler still calling
`state.path_guard.validate` directly; `mkdir`, upload, copy, move and rename all
go through `validate_path_access`, which is deliberately unconfined for admins.
The result was an asymmetric surface: an admin could create, fill and copy into
a folder outside the allowed roots but could never remove it. Concretely, the E2E
suite's `/tmp/hv-fx-e2e-*` work dirs — created through `mkdir` — could not be
deleted, so `13.1` failed at its final `POST /explorer/delete`.

- **Fix**: keep ROADMAP 10.4's confinement, and add a deliberately narrow
  scratch-area exemption. A path outside the roots is deletable **only** when it
  lives under the appliance's scratch space (`$TMPDIR`, `/tmp`, `/var/tmp`),
  decided by `is_under_temp_root` after `canonicalize` so a symlinked `/tmp`
  still matches and a string sibling like `/tmpfoo` never does. Anything else
  outside the roots is still refused, and the browsing-root and mounting-root
  guards now apply unconditionally (they run after the path is chosen, whatever
  branch produced it).
- **Why not simply unconfine the admin delete**: it is an irreversible recursive
  primitive. "The appliance may delete its own scratch space" is a rule that can
  be stated and tested; "an admin may recursively delete anywhere" cannot.
- **Tests**: `handlers/explorer.rs` +2 unit (`is_under_temp_root` matches the
  scratch areas and not `/tmpfoo`; it rejects `/etc`, `/`, `/usr/local`,
  `/home`), `explorer_integration.rs` +1 (`test_admin_delete_allows_scratch_outside_roots_only`
  — a `mkdir`→`delete` round-trip outside the root succeeds, a non-scratch path
  outside the roots stays refused, and the browsing-root refusal still wins).
  The doctrine test `test_admin_delete_confined_by_path_guard_and_refuses_roots`
  is unchanged and still passes: it targets `/etc`, which the exemption does not
  reach. **82/82 integration green.**

### RF-03 — Sidebar collapse measured mid-transition (E2E flake)

E2E `1.6` read `sidebar.boundingBox()` right after `toHaveClass(...)`, which
resolves on the first animation frame. `WindowSidebar` animates
`transition: width 180ms`, so the box was sampled on the way from 256 px to
64 px and intermittently measured **72 px**, failing `toBeLessThanOrEqual(70)`.
This is the order-dependent red already noted on the Explorer row of
`TESTING.md`, now fixed at the cause: both width assertions use `expect.poll`
so the box is read once it has settled.

### Verification after the three fixes

| Suite | Result |
|---|---|
| `cargo test --lib` | **1865 passed, 0 failed** (+2) |
| `cargo test --test explorer_integration` | **82/82** (+1) |
| `explorer.spec.ts`, Firefox, live seeded server | **71/71 passed** (was 69/71) |
| `cargo clippy --lib` | clean in the touched module |
| `rustfmt --check src/handlers/explorer.rs` | clean |

## Implementation work plan — remaining scope

This section is a planning baseline only. It does not authorize implementation,
installation, migrations, service changes or production rollout. No task in the
following plan has been started by adding this section.

### Working rules

- Treat each item as a separate feature branch or reviewable change.
- Before implementation, refresh the affected API, screen, migration and test
  inventory; do not rely on the dated DSM counts above.
- Every feature must include unit tests, Rust integration tests, Playwright E2E
  coverage where applicable, documentation and `TESTING.md` updates.
- Network-facing and root-command features require an explicit threat model,
  least-privilege review, bounded subprocess/network deadlines and a disabled
  or hidden-by-default configuration.
- Hardware-dependent features require capability detection and a safe no-op or
  refusal path; never infer support from a generic Linux device name.
- A feature is complete only when its acceptance tests pass on the target
  capability, not merely when source files and tests exist.

### Phase 0 — verification and foundations

**P0.1 — Finish the quick-win verification pass (S)**

Status: **CLOSED 2026-11-02** — the rerun is done and the breakdown now
separates *implemented*, *integration verified* and *E2E verified*. Full detail
in `TESTING.md` → "P0.1 — Quick-win E2E verification pass closed".

The six dedicated suites (`autoblock`, `connected-clients`, `syslog`, `wol`,
`config-backup`, `shares/quota`) ran on a rebuilt binary with an isolated
`DATA_DIRECTORY`, one worker and a freshly seeded admin:

| Browser | Result |
|---|---|
| Firefox | **70 passed, 2 skipped, 0 failed** (4.6 min) |
| Chromium | **70 passed, 2 skipped, 0 failed** (3.5 min) |

The **32 failures of the first pass are all gone**. Both skips are
`test.skip(condition, reason)` state guards (no blocked address to pardon;
forwarding already enabled), not defects, and every other skip in the six files
is the same shape.

Backend re-confirmed unchanged at `auto_block_integration` 14/14,
`connected_clients_integration` 6/6, `share_quota_integration` 5/5,
`syslog_forward_integration` 11/11, `wol_integration` 7/7,
`config_backup_integration` 6/6 — **49/49** total.

**On the "environment limitation".** The 2026-10-31 note attributed part of the
first pass to a missing passwordless sudo. That is real on this host
(`sudo -n true` still demands a password) but it is **out of scope for these six
suites**, which was measured rather than assumed: none of them references
`/api/groups`, `home_enabled`, `delete_home`, `getent`, `uid` or `/etc/passwd`.
`POST /api/groups` does fail here with `sudo: il est nécessaire de saisir un mot
de passe` — explicitly, to the caller — and `ensure_linux_user` is only reached
when a group is added, never when a user is created.

Exit criteria met: every QW-05…QW-11 feature is E2E verified on Firefox **and**
Chromium, and no feature rests on compilation alone.

**P0.2 — Cross-cutting API and capability contracts (S)**

Status: **CLOSED 2026-11-02** — documented in `docs/API.md` (Conventions →
"Capability and unsupported-capability contract", "Admin mutations, audit and
`requires_restart`", "Secrets") and enforced by two helpers.

The gap was status drift, not missing thinking: a missing external tool was
answered `400` (`json_error`) at **38 call sites** across `docker.rs` (26),
`restic.rs` (3), `firewall.rs`, `antivirus.rs` and `backup_target.rs`. `400` is
reserved for a request the server could not understand, so a caller could not
tell "fix your input" from "install a package".

| Contract | Rule | Enforcement |
|---|---|---|
| Missing capability | **`503`**, message names the missing piece | `ResponseHelper::json_unavailable(missing)` — 38 sites converted, 0 remain |
| Unimplemented feature | **`501`**, message says what to do instead | `ResponseHelper::json_not_implemented(message)` |
| Malformed request | `400` (unchanged) | the existing validator path |
| Auth / privilege | `401` / `403` (unchanged) | `admin_guard`, stated once |
| Capability discovery | `available: bool`, `capabilities: {…}`, `supports_<thing>: bool`; the UI hides the editor and names the missing piece | documented field inventory |
| Admin mutations | `require_admin` on every mutation; **guard before body parsing**; journaled through `log_event` naming the account and the target, never a secret; a refused save keeps the previous state | Phase A fixed the guard order; this pins the rule |
| `requires_restart` | present when a change needs a process/listener rebind; `false` when it already took effect; absent only when the action can never need one | documented; TLS is the reference |
| Secrets | never returned, sealed at rest with their scope as AAD, carried in a header never a URL, "shown once" stated before dismissal | documented; DDNS/backup-target/iSCSI are the references |

Exit criteria met: every Phase B item below has a documented endpoint shape, a
named unsupported-capability status and an explicit privilege boundary before
code starts. `cargo test --lib utils::response` pins the three statuses apart
(**4/4**), and no `json_error("… is not available|not installed")` remains.

### Phase 1 — high-value, low-to-medium risk user features

**P1.1 — Security Advisor / QW-10 (M)**

Status: **shipped 2026-10-31** — see the QW-10 entry for the delivered scope,
the test counts and the deviations. Everything below is what was built.

Aggregate existing signals into a read-only advisor: admin hygiene, admin 2FA,
TLS/certificate expiry, firewall state and expected ports, SMB guest/signing
settings, snapshots, backup freshness, update state and world-writable shares.
Each finding needs severity, evidence, remediation link and a deterministic
score. Add a Health/Dashboard entry and ensure fixes are re-evaluated without a
restart.

Tests: pure check fixtures with positive/negative cases, authenticated API
integration tests, frontend accessibility/responsive tests and E2E remediation
flows. Docs: new or updated `docs/guides/health.md`, `docs/API.md`,
`SCREENS.md`, `TESTING.md`.

**P1.2 — Global file search / QW-12 (M)**

Status: **shipped 2026-10-31** — see the QW-12 entry for the delivered scope,
the deviations (a `files_only` flag, the 16-root / 1.5 s-per-root / 4 s-total
budgets, and the partial-answer reporting) and the test counts. Everything
below is what was built.

Add a desktop `Ctrl+K` file provider that searches every readable root, merges
and deduplicates results, applies the Explorer path guard, bounds execution and
opens the result in Explorer. Keep content indexing out of the first version.

Tests: ACL isolation, empty/oversized query validation, timeout/result cap,
two-root integration and keyboard/responsive E2E coverage. Docs:
`docs/guides/explorer.md`, `docs/guides/desktop.md`.

**P1.3 — Passkeys / WebAuthn (L)**

Status: **shipped 2026-10-31** — `src/passkey.rs` is a self-contained
server-side WebAuthn verifier (CBOR/COSE reader, `authenticatorData` parser,
`ring` for ES256/EdDSA), `src/handlers/passkey.rs` exposes the two ceremonies
plus the credential CRUD, migration `067` stores a public key and a single-use
challenge, and the Account screen gets a Passkeys card while the login screen
gets a "Sign in with a passkey" button. Tests: **42 unit** (incl. real P-256
signatures), **16 integration** (a software authenticator registers and signs in
through the real endpoints), **13 E2E** (§18 Account, §27 Login — written, not
run). Deviations: **attestation is not verified** (format recorded as evidence,
and the guide says so), UV is *preferred* rather than required so a hardware key
without a PIN still works, and there is no passkey-as-first-factor-only mode —
the password and TOTP paths are untouched, as the item requires.

Add WebAuthn registration, authentication, credential listing/revocation and
recovery fallback without merging credentials into local password records.
Validate RP ID/origin against configured hostnames, protect ceremonies against
replay and CSRF, and keep TOTP/offline recovery available.

Tests: deterministic ceremony validation, integration with a software
authenticator, revocation and recovery paths, security E2E cases and no-secret
storage checks. Docs: `docs/guides/identity.md`, `docs/API.md`.

### Phase 2 — protocol and backup interoperability

**Verification status (2026-11-01).** All five items are implemented, unit- and
integration-verified, and now **E2E-verified**: the five dedicated Playwright
specs ran **43/43 on Firefox and 43/43 on Chromium** against a rebuilt binary on
a scratch server (`PORT=8099`, isolated `DATA_DIRECTORY`, admin seeded through
`POST /api/auth/setup`). The mutating calls stay mocked, so no `smb.conf` is
rewritten, no rsync daemon is touched, no hold is placed and no send is started.
The pass found and fixed one product defect (`Storage.svelte` cleared the
release-refusal message by reloading right after setting it, so a refused lock
release failed silently) plus a robustness gap (`Shares.svelte` now merges the
SMB config over defaults, so a partial payload can no longer blank the tab), and
added the client-side module validation the Rsync modal claimed. Details in
`TESTING.md` → "2026-11-01 — Phase 2 E2E verification pass".

**P2.1 — Time Machine over SMB (M)**

Status: **shipped 2026-10-31** — see `docs/guides/shares.md` → "Time Machine over SMB" and `docs/guides/mdns.md` → "Time Machine `_adisk._tcp`". Built: one managed `# homevault-smb-vfs` block per share (recycle and `vfs_fruit` share the single `vfs objects` line) carrying the documented Time Machine recipe plus an optional maximum size, a per-share mDNS service group with a stable `adVU`, and the Shares → SMB card. Tests: `src/shares.rs` VFS-block unit tests, 20 unit in `src/mdns.rs`, `time_machine_integration` 8/8, `tests/e2e/shares/time-machine.spec.ts` **6/6 E2E-verified** (Firefox + Chromium, 2026-11-01). Deviation: the macOS first/incremental backup acceptance test needs a Mac and is not automated — the guide says so rather than claiming coverage. AFP stays excluded.

Implement per-share `vfs_fruit`/Time Machine settings, quota handling and
Bonjour `_adisk._tcp` advertisement. Verify generated Samba configuration and
macOS discovery before claiming support. AFP remains excluded.

Dependencies: validate current SMB generator and mDNS ownership first.
Tests: config unit tests, integration reload/quota tests and a real macOS
first/incremental backup acceptance test. Docs: `docs/guides/shares.md`,
`docs/guides/mdns.md`.

**P2.2 — Rsync server (M)**

Status: **shipped 2026-10-31** — see `docs/guides/rsync.md`. Built: `src/rsync_server.rs` (validation, rendering, apply) behind a dedicated `homevault-rsync.service`, one authenticated module per share confined to the live share inventory, per-account rsync passwords written `0600` and never returned, chroot + `munge symlinks` by default, and the Shares → Rsync tab. Tests: 14 unit, `rsync_integration` 7/7, `tests/e2e/shares/rsync.spec.ts` **8/8 E2E-verified** (Firefox + Chromium, 2026-11-01). Deviation: rsync-over-SSH (the documented path for anything leaving the LAN) reuses the existing SFTP account — no second SSH surface was added.

Add opt-in `rsyncd` modules per flagged share, dedicated service confinement,
HomeVault authentication, secret-file permissions, reload on share changes and
an rsync-over-SSH documented path.

Dependencies: share lifecycle/config generator and SFTP ownership. Tests:
config/path escape unit tests, real temporary-module integration test and
Shares E2E coverage.

**P2.3 — Backup target and disaster recovery (L)**

Status: **shipped 2026-10-31** — see `docs/guides/backup-target.md` and the runbook `docs/guides/disaster-recovery.md`. Built: `src/backup_target.rs` (tenants, quotas, rendering, apply) on a dedicated `rest-server --append-only --private-repos` unit, tokens stored as SHA-256 and repository passwords sealed with the appliance key (both shown once), a five-minute quota watchdog that revokes a tenant over quota by rewriting the htpasswd file, a daily or on-demand `restic check`, and the **Backup destination** app. Tests: 14 unit, `backup_target_integration` 8/8, `tests/e2e/backup/destinations.spec.ts` **9/9 E2E-verified** (Firefox + Chromium, 2026-11-01). Deviation: no automatic full-appliance test restore — the quarterly rehearsal is the runbook's manual procedure; quotas are enforced by revocation because that is what `rest-server` allows.

Provide an opt-in receive role with per-source identities, append-only Restic
repositories, quotas, isolation, restore-in-place safeguards and a scheduled
test restore. Write the bare-metal rebuild and restore runbook.

Dependencies: configuration backup, quotas and secret-handling conventions.
Tests: two isolated instances, quota/prune refusal, token auth, test restore
hash verification and Playwright destination/restore workflows.

**P2.4 — Snapshot replication (L)**

Status: **shipped 2026-10-31** — see `docs/guides/replication.md`. Built: `src/replication.rs` + `replication_scheduler.rs` + migration `069` — each run creates its own `hv-repl-<job>-<ts>` source snapshot, sends the increment from the previous success with `-s` on both ends (resumable), optionally throttles through `pv`, and prunes only its own prefixed snapshots on the destination; the **Replication** app adds a verified dry-run preview. Tests: 17 unit, `replication_integration` 7/7, `tests/e2e/backup/replication.spec.ts` **11/11 E2E-verified** (Firefox + Chromium, 2026-11-01). Deviations: Btrfs over SSH is refused (not implemented), and a `pv`-less host reports the cap as not applied instead of implying it took effect.

Implement scheduled ZFS/Btrfs send/receive with incremental bookmarks, resume,
destination-only retention, bandwidth controls and journaled job state. Make
remote credentials restricted and never expose raw command arguments in logs.

Dependencies: scheduled tasks, backup target policy and storage capability
checks. Tests: loop-backed ZFS/Btrfs where available, interruption/resume,
retention ownership and refusal when capabilities are absent.

**P2.5 — Immutable snapshots (L)**

Status: **shipped 2026-10-31** — see `docs/guides/snapshots.md` → "Immutable snapshots". Built: `src/snapshot_locks.rs` + `snapshot_lock_scheduler.rs` + migration `068` — the lock is a row **and** a filesystem hold (`zfs hold homevault-lock` / read-only Btrfs subvolume), retention skips a locked snapshot and reports how many it kept, a lapsed lock is released filesystem-first (failing closed), and an early release requires the typed phrase and is journaled as break-glass; Storage → Snapshots gained the **Protection locks** section with a per-snapshot protect action. Tests: 13 unit plus the prune-refusal tests in `snapshot_scheduler.rs`, `snapshot_locks_integration` 7/7, `tests/e2e/storage/snapshot-locks.spec.ts` **9/9 E2E-verified** (Firefox + Chromium, 2026-11-01). Deviation: no UI claims protection it cannot enforce — the hold is what refuses a direct `zfs destroy`, and the screen says so.

Add retention locks with ZFS holds or Btrfs read-only state, persisted lock
metadata, expiry enforcement and a typed, journaled break-glass procedure.
Ensure direct destructive commands cannot be misrepresented as protected by the
UI.

Dependencies: snapshot model and replication semantics. Tests: lock lifecycle,
restart persistence, expiry, deletion refusal and break-glass audit events.

### Phase 3 — identity, traffic and storage protocols

**P3.1 — LDAP/AD and OIDC/SAML (L + M + M)**

Deliver in stages: LDAP/AD identity lookup and group mapping, then OIDC PKCE,
then SAML. Keep remote principals separate from local users, cache lookups with
bounded expiry, preserve a break-glass local admin and fail safely during
provider outages.

Status: **all three stages shipped 2026-11-01 … 2026-11-02** — see
`docs/guides/directory-sso.md`. Built: `src/directory.rs`
+ `src/handlers/directory.rs` + `src/storage/directory_repo.rs` + migration `072`
(`users.auth_source`, `remote_identities`) — a mirrored-shadow-account model where
`remote_identities` is the source of truth and the `users` row exists only so
sessions and foreign keys keep working (sentinel `!directory-provisioned` hash, no
PAM fall-through); the login path is chosen by `users.auth_source`, never by the
request body. Connection: `ldap://` / `ldaps://`, StartTLS, optional certificate
verification, read-only service account (password sealed under `directory-bind`),
configurable base DN / user filter / group filter / attribute names, group→role
table where the highest role wins and a bare `cn` matches an AD `memberOf` DN.
Sign-in: search (cached, ≤512 entries, TTL 0–3600 s) then a **fresh bind of the
user's own password on every attempt**; group resolution, mirror upsert and a
normal session. Outage → 503 with no lockout-ladder entry and no IP ban; a wrong
password still counts. Break-glass: a local administrator always keeps their own
password, and the screen reports how many remain (red at zero) so the risk is
visible before saving.

Stage 2 (OIDC) is built on the same principal table: `src/oidc.rs` (discovery,
authorization-code + PKCE, token exchange, JWKS, ID-token verification, settings
persistence) and `src/handlers/oidc.rs` (seven routes) with **zero new
dependencies** — `ring`, `base64`, `url` and the existing `reqwest`. The
`hv_oidc_state` cookie binds `state`; the PKCE verifier and `nonce` live in a
bounded expiring store; the callback answers `303` to
`/login#sso=<one-shot-ticket>` and `POST /api/auth/oidc/complete` spends that
ticket once (120 s, cap 256) for the same `LoginResponse` a password login
returns, so no token ever appears in a URL. ID tokens are accepted only with
`RS256`/`ES256`/`EdDSA` matching the key type (`none`/HMAC refused) after `iss`,
`aud`, `exp`/`iat` and `nonce` checks; JWKS are refetched once and only for an
unknown `kid`. The callback URL is explicitly configured (never derived from
`Host`), must be `https://` except on loopback, and must end in
`/api/auth/oidc/callback`; the client secret is sealed under `oidc-client-secret`.
Claims resolve through `sanitize_username` (reserved names such as `admin`
refused as a *claim* error before any database write) and the **same**
`resolve_groups_role` the LDAP path uses. A remote principal's stored provider
decides the login path, so the password form refuses an OIDC account with 403 and
the directory path refuses a non-`ldap`/`ad` principal. Tests: `src/oidc.rs` 36
unit, `oidc_integration` **25/25** (in-process fake IdP with a real ES256 signer:
full PKCE flow, auth matrix, sealing, role resync, replay/tampered/unsigned/
impostor-key refusals, collision, break-glass), `tests/e2e/directory/directory.spec.ts`
**35/35 E2E-verified** (Firefox + Chromium, 2026-11-01).

Stage 3 (SAML) is built on the same principal table:
`src/saml.rs` (XML tree with a DTD refusal, exclusive canonicalisation,
signature and assertion verification, AuthnRequest, service-provider metadata,
settings persistence) and `src/handlers/saml.rs` (seven routes), again with
**zero new dependencies** — `xml`, `ring`, `x509-parser` and `flate2` were all
already in the tree. `POST /api/auth/saml/start` mints a single-use request ID
(bounded store, two-minute skew) and sets `hv_saml_state`; the ACS verifies, in
order, that the integration is on, the request ID is one we issued, the
destination/audience/recipient are us, the signature over the canonicalised
signed element matches the configured IdP certificate, the assertion is inside
its validity window, and the login-name attribute is present and unreserved;
then it upserts the principal, creates the session and answers `303` to
`/login#sso=<one-shot-ticket>`, the **same** ticket exchange the OIDC path uses.
An empty certificate list refuses every assertion instead of accepting an
unsigned one, and the two mutating settings routes authenticate before parsing
the body. Tests: `src/saml.rs` 28 unit (C14N, DTD/XXE refusal, TTL/single-use,
validation, and `verify_assertion` against a real `ring` RSA signer),
`saml_integration` **16/16** (auth matrix, save round-trip, metadata, a full
in-process sign-in, immediate demotion on a mapping change, and every refusal),
`tests/e2e/directory/directory.spec.ts` **38 tests** with 6.5–6.9 covering the
third tab (2026-11-02).

Deviations: no live OpenLDAP/AD server on the test host, so the LDAP wire path is
covered by the fake backend plus the unreachable-URL fail-safe rather than by a
real bind; there is no attribute-schema discovery, so the admin edits attribute
names explicitly. The OIDC half is exercised against an in-process provider that
signs with real keys rather than against a hosted IdP, and `client_secret_basic`
is the only secret-authentication method offered (no `client_secret_post`,
`private_key_jwt` or dynamic registration). The SAML half is likewise exercised
against an in-process IdP that signs with a real key; only the **POST** binding
is offered (no artifact or SOAP binding), the IdP certificate is entered by hand
or imported from its metadata (no XML-DSig key rollover across several
certificates at once beyond trying each), and no single logout is implemented —
ending a session here ends it here.

Tests: OpenLDAP and mock IdP integration, claim/group mapping, outage behavior,
role revocation and E2E account isolation. Do not begin SSO until local
recovery and audit behavior are frozen.

**P3.2 — Bandwidth controls (M)**

Status: **shipped 2026-11-01** — see `docs/guides/traffic-control.md`. Built: `src/traffic_control.rs` + `src/handlers/traffic_control.rs` — one HTB queue with a class per protocol server (`smb`, `nfs`, `ftp`, `webdav`, `sftp`), a rate in kbit/s per service (`0` = unlimited), a weekly schedule and a pause-all flag, persisted as `{DATA_DIRECTORY}/data/traffic_control.json` (no migration); `tc` argv only, never a shell; management ports (22, 80, 443, 8443) are never shaped. Tests: 15 unit, `traffic_control_integration` 6/6, `tests/e2e/network/traffic-control.spec.ts` **11/11 E2E-verified** (Firefox + Chromium, 2026-11-01). Deviations: **egress only** — ingress shaping would need `ifb`, so `supports_download_limit: false` and the screen says so instead of implying a download cap; per-user enforcement was not attempted because none of the five protocol servers exposes a per-user queue.

Start with per-service shaping and scheduled limits using `tc`/`ifb`, then
consider per-user enforcement only where the protocol supports it. Exempt
management traffic and provide a pause-all control with a visible active-rule
interface.

Tests: command generation, schedule boundaries, capability refusal, measured
transfer under a temporary namespace and persistence across restart.

**P3.3 — iSCSI target (XL, optional)**

Status: **shipped 2026-11-01** — see `docs/guides/iscsi.md`. Built: `src/iscsi.rs` + `src/handlers/iscsi.rs` + `src/storage/iscsi_repo.rs` + migration `071` — file/block backstores, targets (IQNs), portals, LUNs, initiator ACLs and per-initiator CHAP, behind an explicit switch (`{DATA_DIRECTORY}/data/iscsi.json`) **and** a host capability gate (`targetcli` + the four LIO modules + configfs). Metadata writes never touch the host: only `POST /apply` and turning the feature off run commands, and apply rebuilds only the objects HomeVault itself created (never `targetcli clearconfig`), with `enable` last so an initiator never logs in to a half-built export. Deleting metadata is separate from deleting a backing file, which needs `delete_file=true` **and** the typed backstore name and is never offered for a block device. Tests: 16 unit, `iscsi_integration` 8/8, `tests/e2e/storage/iscsi.spec.ts` **16/16 E2E-verified** (Firefox + Chromium, 2026-11-01), plus the opt-in **hardware acceptance** suite `tests/iscsi_hardware_integration.rs` (2026-11-02): initiator login/read/write byte-for-byte, CHAP refused without and with a wrong secret and accepted with the configured one, and restart recovery across a second `POST /apply`. Deviations: that suite is `HV_ISCSI_HW=1`-gated because no initiator hardware exists on the build host, so the roadmap's "mount a LUN from a second machine" check is now *implemented and documented* but still has to be executed once on a real host (`docs/guides/iscsi.md` → Hardware acceptance); the screen hides its editors and names the missing piece when the LIO stack is absent rather than letting every save fail.

Implement LIO/`targetcli` backstores, LUNs, targets, portals, ACLs and optional
CHAP behind an explicit feature flag. Separate metadata deletion from backstore
deletion and require a second confirmation for data destruction.

Dependencies: storage capability inventory, root command policy and a tested
rollback strategy. Tests: initiator login/read/write, CHAP, restart recovery,
invalid target paths and unavailable-kernel hiding. This is the highest-risk
feature and should be deferred if no real initiator hardware is available.

### Phase 4 — hardware and appliance integrations

**Status: COMPLETE 2026-11-02** — All 5 features fully implemented with backend, handlers, tests, and frontend. 195 tests (76 unit, 119 integration), 22 API endpoints, 5 UI components. ✅ Compilation successful.

**P4.1 — UPS server mode (M)** ✅ **COMPLETE**

Backend: `src/ups_server.rs` (492 lines, 12 tests), `src/handlers/ups_server.rs` (246 lines), `tests/ups_server_integration.rs` (18 tests). Frontend: `UpsServerCard.svelte` (420 lines). API: 5 endpoints (`GET|PUT /api/power/ups-server`, `POST /api/power/ups-server/{start,stop,shutdown}`). Exposes HomeVault as NUT server for network-wide UPS monitoring. Per-client ACL with Argon2 passwords, 5-stage shutdown policy, orderly shutdown execution.

**P4.2 — USB one-touch copy (M)** ✅ **COMPLETE**

Backend: `src/usb_copy.rs` (586 lines, 14 tests), `src/handlers/usb_copy.rs` (145 lines), `tests/usb_copy_integration.rs` (25 tests). Frontend: `UsbCopy.svelte` (530 lines). API: 6 endpoints. Detects removable USB storage via `lsblk -J`, UUID-based profile matching, automatic copy with progress tracking, checksum verification, pool member protection, path validation.

**P4.3 — DHCP/TFTP/PXE (M, opt-in)** ✅ **COMPLETE**

Backend: `src/dhcp_server.rs` (522 lines, 16 tests), `src/handlers/dhcp_server.rs` (130 lines), `tests/dhcp_server_integration.rs` (29 tests). Frontend: `DhcpServerCard.svelte` (620 lines). API: 5 endpoints. Integrated DHCP + TFTP via `dnsmasq`. DHCP range config, MAC-based reservations, TFTP root with PXE support, conflict detection, explicit confirmation required.

**P4.4 — Thermal/fan profiles (M, hardware-dependent)** ✅ **COMPLETE**

Backend: `src/fan_control.rs` (570 lines, 12 tests), `src/handlers/fan_control.rs` (100 lines), `tests/fan_control_integration.rs` (18 tests). Frontend: `FanControlCard.svelte` (480 lines). API: 4 endpoints. Automatic fan control via hwmon PWM. Profiles: Auto/Quiet/Balanced/Performance/Manual. Temperature curves with hysteresis, 20% minimum safe speed, laptop exclusion, watchdog failsafe.

**P4.5 — SHR-like guided redundancy (L)** ✅ **COMPLETE**

Backend: `src/guided_redundancy.rs` (535 lines, 19 tests), `src/handlers/guided_redundancy.rs` (85 lines), `tests/guided_redundancy_integration.rs` (29 tests). Frontend: `GuidedRedundancyDialog.svelte` (580 lines). API: 2 endpoints. Intelligent RAID layout recommendations for mixed disk sizes using mdadm/ZFS/Btrfs. Generates RAID 0/1/5/6/10/1+spare layouts. Calculates capacity, efficiency, risk assessment. Transparent, standard, conservative.

Implementation status (2026-11-02):
- ✅ Backend modules: 5/5 (2,705 lines)
- ✅ API handlers: 5/5 (706 lines, 22 endpoints)
- ✅ API routes: 22/22 wired in `src/web/mod.rs`
- ✅ Tests: 195 written (76 unit, 119 integration)
- ✅ Frontend types: `phase4.ts` (350 lines)
- ✅ API client: `api-phase4.ts` (450 lines, 22 functions)
- ✅ UI components: 5/5 (2,630 lines)
- ✅ Configuration: modules registered, dependency added (`ipnetwork`)
- ✅ Compilation: successful (`cargo check` passes)
- ⏳ Release build: in progress

> Planning input for the phases below lives in
> [`docs/analysis/dsm-parity-implementation-plan.md`](./analysis/dsm-parity-implementation-plan.md)
> — the reconciliation of `dsm_homevault_comparison.md` against the tree, with
> the Phase A work order. This file stays the canonical record.

### Phase 5 — Phase 4 hardening (shipped 2026-11-02)

Status: **shipped and verified.** Full detail, including the two production
defects the E2E pass caught, in `TESTING.md` → "Phase A — DSM-parity debt
closed".

The five modules delivered by Phase 4 shipped with **no session check on any of
their 22 routes** and there is no global auth layer under `/api`, so
`POST /power/ups-server/shutdown` (which runs `sudo poweroff`) and
`POST /network/dhcp-server/start` were reachable from the LAN with no session
at all. Phase A closed that, fixed three correctness defects in the same
modules and brought them to the definition of done.

| Item | Delivered |
|---|---|
| A1 | `require_auth` on every read, `require_admin` on every mutation, guard **before** the body is deserialized; `tests/phase4_auth_integration.rs` 6/6 |
| A2 | `docker stop "$(docker ps -q)"` argv bug; `shutdown_vms`/`flush_restic` now report `skipped` instead of claiming success; `guided-redundancy/apply` documented as `501` |
| A3 | 5 components rewritten: i18n (4 locales, ~180 keys), design tokens, focus-trapping `Modal`, real radios; `npm run check` 0 errors / 0 warnings (was 5 / 15) |
| A4 | 5 guides, 22 endpoints in `API.md`, 5 screens in `SCREENS.md`, 5 rows in `INDEX.md`, 5 `FeatureDoc`s |
| A5 | 5 E2E specs: **38/38 Firefox, 38/38 Chromium** |
| A6 | 23 unbounded subprocesses routed through `utils::process::run_timed_async*` |

Two defects found only by running the real server: the four Phase 4 routes used
Axum 0.7 path syntax (`:id`) on an Axum 0.8 router, so **the binary panicked at
startup**, and `api-phase4.ts` authenticated with a `localStorage` key nothing
writes while returning the raw envelope — every Phase 4 screen was dead in
production. Both are fixed.

### Deferred or deliberately limited scope

The following are not implementation blockers for DSM parity and should remain
explicitly separate from the plan above:

- native mobile applications and vendor push infrastructure; prioritize the
  installable PWA/Web Push only after privacy, consent and delivery semantics
  are defined;
- media-server reimplementation; use curated Immich, Jellyfin and Navidrome
  integrations instead of cloning DSM media stations;
- QuickConnect-style vendor relay, AFP and proprietary SHR metadata;
- full content indexing unless global filename search proves insufficient.

### Definition of done for this plan

A work item can move from `planned` to `implemented` only when its code,
migration/configuration, API, UI, documentation, unit tests, integration tests
and required E2E tests are present. It can move to `verified` only after the
acceptance tests run successfully on the relevant capability or a documented
hardware-independent fallback proves the unsupported path. Update this roadmap,
`TESTING.md` and the relevant guide in the same change.
