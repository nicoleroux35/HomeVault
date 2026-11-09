# HomeVault Developer Documentation

HomeVault turns any small computer into a private NAS: a self-hosted home cloud
for any kind of data, reachable from any device with a browser, on a minimal
hardware footprint (1 core, 1 GB of RAM, 8 GB of system disk). The product goal
is to stay **simple, reliable and robust** — that is what the invariants in this
documentation protect.

Technically it is a Rust + Axum backend with an embedded SQLite database, and a
Svelte 5 + TypeScript single-page frontend that renders a desktop-like windowing
UI.

This index is the single entry point. Every canonical doc is listed below with a
one-line description. All docs are in English (the product UI defaults to French).

> **Relationship to `CLAUDE.md`:** `CLAUDE.md` at the repo root is the
> operational rulebook — repo hygiene, the testing rules, the compliance
> baseline, and the **engineering invariants** (what is easy to break again).
> It defers to `docs/` for *how things work*
> and points each area at its guide here. When a *fact* disagrees, fix
> whichever is stale; when a *rule* disagrees, `CLAUDE.md` wins.

## Start here

- [Architecture](./ARCHITECTURE.md) — how backend and frontend fit together, how to
  build/run, the development workflow.
- [API Contract](./API.md) — every HTTP endpoint grouped by area, with the auth
  model (auth vs. admin) and request/response conventions. Also documents the
  generated OpenAPI 3 document served to administrators at
  `GET /api/openapi.json`.
- [Backend Structure](./BACKEND.md) — handlers, storage layer, services, middleware,
  app state, server startup.
- [Frontend Structure](./FRONTEND.md) — gate routes and the desktop window
  registry, API client, stores, shared components, windowing.
- [Internationalization (i18n)](./I18N.md) — locales, translation keys, the
  `api.getTranslations` endpoint, how to add a string.

## Application / area guides

Each guide lists the backend handlers, the Svelte route component, the API client
methods, and notable invariants.

| Area | Guide |
|------|-------|
| Authentication, 2FA, offline recovery code, sessions | [guides/auth.md](./guides/auth.md) |
| First-run Setup Wizard | [guides/setup-wizard.md](./guides/setup-wizard.md) |
| Users management | [guides/users.md](./guides/users.md) |
| Groups management | [guides/groups.md](./guides/groups.md) |
| "My Account" (self-service profile) | [guides/account.md](./guides/account.md) |
| Dashboard & widgets | [guides/dashboard.md](./guides/dashboard.md) |
| Storage (disks, partitions, virtual disks, snapshots) — **manages** all mdadm/ZFS/Btrfs virtual disks (scrub, rebuild, grow, destroy) | [guides/storage.md](./guides/storage.md) |
| Disk Cache Tier (cache pool, mover, shares) | [guides/cache.md](./guides/cache.md) |
| Disk Encryption (LUKS2) | [guides/encryption.md](./guides/encryption.md) |
| RAID — **creation-only** wizard (physical disks → one tracked async job → one virtual disk; spares, degraded slots) | [guides/raid.md](./guides/raid.md) |
| File Explorer (incl. private file tags, reflink/instant copy, trash retention, transfer tray/progress detail/history, resumable-upload durability, on-demand SHA-256/SHA-512/BLAKE3 checksums) | [guides/explorer.md](./guides/explorer.md) |
| Network locations — CIFS/NFS remote mounts browsable in the Explorer | [guides/remote-mounts.md](./guides/remote-mounts.md) |
| File tags (private per-user labels, tag-filtered search) | [guides/file-tags.md](./guides/file-tags.md) |
| SNMP agent (the appliance's own `snmpd`: v2c community or v3 authPriv, loopback/LAN scope) | [guides/snmp-agent.md](./guides/snmp-agent.md) |
| Shares (SMB/NFS/FTP/WebDAV + ACLs) | [guides/shares.md](./guides/shares.md) |
| Network (interfaces, WiFi, static routes, diagnostics) | [guides/network.md](./guides/network.md) |
| mDNS / Zeroconf (`homevault.local`, share discovery) | [guides/mdns.md](./guides/mdns.md) |
| Backup / Restic (repos, snapshots, schedules) | [guides/backup-restic.md](./guides/backup-restic.md) |
| AI Assistant (Chat, Ollama) | [guides/chat.md](./guides/chat.md) |
| RAG Knowledge Base (embeddings, semantic search) | [guides/rag.md](./guides/rag.md) |
| External Agents (MCP server, tokens, granular permissions) | [guides/mcp.md](./guides/mcp.md) |
| Terminal (native PTY over WebSocket) | [guides/terminal.md](./guides/terminal.md) |
| Docker (containers, images, networks, compose) | [guides/docker.md](./guides/docker.md) |
| Cloud Sync (rclone) | [guides/cloud-sync.md](./guides/cloud-sync.md) |
| Syncthing | [guides/syncthing.md](./guides/syncthing.md) |
| Downloader (aria2) | [guides/downloader.md](./guides/downloader.md) |
| Snapshots (pool rollback) | [guides/snapshots.md](./guides/snapshots.md) |
| Rescue tools (ddrescue, TestDisk, PhotoRec, mdadm) | [guides/rescue.md](./guides/rescue.md) |
| VPN (WireGuard, OpenVPN) | [guides/vpn.md](./guides/vpn.md) |
| Virtual Machines (Incus, KVM/QEMU — create, clone, snapshots, console) | [guides/vm.md](./guides/vm.md) |
| Wallpapers | [guides/wallpapers.md](./guides/wallpapers.md) |
| Avatars (DiceBear, generated offline) | [guides/avatars.md](./guides/avatars.md) |
| Disk quotas (per-user home-directory limit, and per-share size quotas) | [guides/quotas.md](./guides/quotas.md) |
| Transfer quotas (upload/download, per user/group) — incl. the read-only **Quotas** app | [guides/transfer-quotas.md](./guides/transfer-quotas.md) |
| Security (sessions, login history, CSP, brute-force, login auto-block, connected clients) | [guides/security.md](./guides/security.md) |
| Identity (trusted devices, rescue e-mail, account recovery, **passkeys/WebAuthn**) | [guides/identity.md](./guides/identity.md) |
| Settings (general, security, appearance, widgets, notifications, AI, monitoring, system — 8 rail groups, incl. the outbound proxy and the full configuration backup/restore) | [guides/settings.md](./guides/settings.md) |
| Email OAuth2 (Gmail / Microsoft 365, sealed refresh token, no app password) | [guides/email-oauth.md](./guides/email-oauth.md) |
| Themes | [guides/themes.md](./guides/themes.md) |
| Desktop shell & windowing | [guides/desktop.md](./guides/desktop.md) |
| Activity taskbar (unified background-job feed) | [guides/activity.md](./guides/activity.md) |
| Apps Center | [guides/apps-center.md](./guides/apps-center.md) |
| Server control (restart/shutdown) | [guides/server-control.md](./guides/server-control.md) |
| Health monitoring (incl. the Security Advisor) | [guides/health.md](./guides/health.md) |
| Global file search (⌘K) | [guides/global-search.md](./guides/global-search.md) |
| Time Machine over SMB (`vfs_fruit`, `_adisk._tcp`) | [guides/shares.md](./guides/shares.md#time-machine-over-smb-2026-10-31-roadmap-p21) |
| Rsync server (opt-in modules, per-user passwords, SSH alternative) | [guides/rsync.md](./guides/rsync.md) |
| Backup destination (receive restic, append-only tenants, quotas) | [guides/backup-target.md](./guides/backup-target.md) |
| Snapshot replication (ZFS/Btrfs, local or SSH, destination-only retention) | [guides/replication.md](./guides/replication.md) |
| Traffic control (per-service bandwidth limits, weekly schedule, pause-all) | [guides/traffic-control.md](./guides/traffic-control.md) |
| iSCSI exports (disks over the network, CHAP, apply-now) | [guides/iscsi.md](./guides/iscsi.md) |
| Directory sign-in and single sign-on (LDAP/AD, OpenID Connect, SAML, group→role mapping, break-glass local admin) | [guides/directory-sso.md](./guides/directory-sso.md) |
| Immutable snapshots (retention locks, holds, break-glass) | [guides/snapshots.md](./guides/snapshots.md#immutable-snapshots-2026-10-31-roadmap-p25) |
| Disaster recovery (runbook: config restore, rebuild, restic restore) | [guides/disaster-recovery.md](./guides/disaster-recovery.md) |
| Notifications (persistent center, health/backup alerts, email, webhook, What's New) | [guides/notifications.md](./guides/notifications.md) |
| Antivirus (ClamAV) | [guides/antivirus.md](./guides/antivirus.md) |
| Anti-Ransomware (honeypots, rate detection) | [guides/ransomware.md](./guides/ransomware.md) |
| Firewall (ufw) | [guides/firewall.md](./guides/firewall.md) |
| HTTPS / TLS (built-in listener, self-signed, manual and automated certs) | [guides/tls.md](./guides/tls.md) |
| Certificate automation (ACME, HTTP-01 / DNS-01, renewal worker) | [guides/acme.md](./guides/acme.md) |
| Dynamic DNS (DuckDNS, dynv6, Cloudflare, OVH DynHost, Namecheap, deSEC, custom; change-only updates) | [guides/ddns.md](./guides/ddns.md) |
| Reachability advisor (ordered checks with their fixes) | [guides/reachability.md](./guides/reachability.md) |
| System Journal (log viewer, auto-cleanup) | [guides/journal.md](./guides/journal.md) |
| Widgets (desktop monitor catalog) | [guides/widgets.md](./guides/widgets.md) |
| Web shortcuts (pinned desktop links, SSRF-guarded metadata fetch) | [guides/shortcuts.md](./guides/shortcuts.md) |
| Scheduled Tasks (generic cron engine: commands, snapshots, SMART, power) | [guides/scheduled-tasks.md](./guides/scheduled-tasks.md) |
| Metrics (historical CPU/memory/load/network/temperature charts) | [guides/metrics.md](./guides/metrics.md) |
| Power (host power management, CPU governor, disk spindown, Wake-on-LAN) | [guides/power.md](./guides/power.md) |
| SMS alerts (provider-agnostic gateway, E.164 numbers, one-segment composition) | [guides/sms.md](./guides/sms.md) |
| ISO / IMG mounting (read-only loop mounts, browsable as a folder) | [guides/iso-mount.md](./guides/iso-mount.md) |
| DHCP / TFTP / PXE server (address range, static reservations, boot files, live leases) | [guides/dhcp.md](./guides/dhcp.md) |
| Local DNS server (local names, answered domains, upstream forwarders, DHCP advertisement — the same `dnsmasq`) | [guides/local-dns.md](./guides/local-dns.md) |
| Reverse proxy (user-defined host → upstream rules, one host-routed listener, cross-host sign-in gate) | [guides/reverse-proxy.md](./guides/reverse-proxy.md) |
| Router port forwarding (UPnP-IGD: list/add/remove mappings, external address, reversible and confirmed) | [guides/upnp.md](./guides/upnp.md) |
| Web Push notifications (per-device consent, VAPID, RFC 8291 encryption, revocation, best-effort delivery) | [guides/web-push.md](./guides/web-push.md) |
| Content search (opt-in FTS5 index, text files only, ACL-filtered results, bounded index, delete-my-index) | [guides/content-index.md](./guides/content-index.md) |
| USB one-touch copy (copy rules, device matching, checksum verification, safe eject) | [guides/usb-copy.md](./guides/usb-copy.md) |
| Fan and thermal control (hwmon channels, profiles, 20 % safety floor, failsafe) | [guides/fan-control.md](./guides/fan-control.md) |
| UPS server mode (NUT server, client allowlist, orderly shutdown, skipped stages) | [guides/ups-server.md](./guides/ups-server.md) |
| Guided redundancy (RAID layout suggestions, capacity/efficiency/risk) | [guides/guided-redundancy.md](./guides/guided-redundancy.md) |
| Design system (single UI identity — tokens, components, page anatomy) | [guides/design-system.md](./guides/design-system.md) |
| Development workflow (multi-instance `.local`, real-VM tests, client contract) | [guides/development.md](./guides/development.md) |

## Design & audit material (`docs/design/`)

Not authoritative (see [CLAUDE.md](../CLAUDE.md)): `docs/FRONTEND.md` +
`frontend/src/lib/styles/theme/tokens.css` win. Kept for history and follow-up:

- [legacy-design-reference.md](./design/legacy-design-reference.md) — the
  superseded "Cove"/MD3-era design specification, banner-marked LEGACY. Read only
  to trace a historical decision.
- [ui-audit-tasklist.md](./design/ui-audit-tasklist.md) — the 2026-09-23
  full-app design-audit backlog (17 open tasks, worked top to bottom, one
  commit-sized change each); referenced by the
  [design-system guide](./guides/design-system.md).

## Planning material (`docs/analysis/`)

Not authoritative — `ROADMAP-DSM.md` is the canonical roadmap. These files
record how a roadmap item was derived, so the reasoning is not lost:

- [dsm-parity-implementation-plan.md](./analysis/dsm-parity-implementation-plan.md)
  — reconciles `dsm_homevault_comparison.md` (a DSM 7 capability comparison)
  against the actual tree: which "missing" features already shipped, which gaps
  are real, and the Phase A–E work order. Feeds
  [ROADMAP-DSM.md](../ROADMAP-DSM.md) → Phase 5.

## Doc conventions

- Top-level docs use `UPPERCASE.md` (`ARCHITECTURE.md`, `API.md`, …); everything
  under `guides/` and `design/` uses `kebab-case.md`. All docs are English-only.
- Each doc: one-line purpose, overview, relevant endpoints/components with
  `path:line` references, request/response shapes, edge cases/invariants, and
  links to related docs.
- Endpoint paths are relative to the SPA base `/api` (e.g. `/api/auth/login` is
  written as `auth/login`).
- Counts and file lists in `docs/` are produced by actually grepping the tree,
  not from memory.
- Every guide listed above has a matching RAG `FeatureDoc` in
  `src/rag_indexers.rs` (except `design-system.md` and `development.md`, which
  document tokens/tooling rather than user-facing features and are excluded in
  `DEVELOPER_ONLY_GUIDES`) — when you add a guide, add its `FeatureDoc` in the
  same change.
