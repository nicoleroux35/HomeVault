# HomeVault Developer Documentation

HomeVault is a self-hosted NAS / home-server management platform: a Rust + Axum
backend with an embedded SQLite database, and a Svelte 5 + TypeScript single-page
frontend that renders a desktop-like windowing UI.

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
  model (auth vs. admin) and request/response conventions.
- [Backend Structure](./BACKEND.md) — handlers, storage layer, services, middleware,
  app state, server startup.
- [Frontend Structure](./FRONTEND.md) — routes, API client, stores, shared
  components, desktop windowing.
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
| Storage (disks, partitions, virtual disks, snapshots) — **manages** all mdadm/ZFS/Btrfs virtual disks | [guides/storage.md](./guides/storage.md) |
| Disk Cache Tier (cache pool, mover, shares) | [guides/cache.md](./guides/cache.md) |
| Disk Encryption (LUKS2) | [guides/encryption.md](./guides/encryption.md) |
| RAID — **creation-only** wizard (physical disks → one virtual disk) | [guides/raid.md](./guides/raid.md) |
| File Explorer | [guides/explorer.md](./guides/explorer.md) |
| Shares (SMB/NFS/FTP/WebDAV + ACLs) | [guides/shares.md](./guides/shares.md) |
| Network (interfaces, WiFi, diagnostics) | [guides/network.md](./guides/network.md) |
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
| Virtual Machines (Incus, KVM/QEMU) | [guides/vm.md](./guides/vm.md) |
| Wallpapers | [guides/wallpapers.md](./guides/wallpapers.md) |
| Avatars (DiceBear, generated offline) | [guides/avatars.md](./guides/avatars.md) |
| Disk quotas (per-user home-directory size limit) | [guides/quotas.md](./guides/quotas.md) |
| Transfer quotas (upload/download, per user/group) — incl. the read-only **Quotas** app | [guides/transfer-quotas.md](./guides/transfer-quotas.md) |
| Security (sessions, login history) | [guides/security.md](./guides/security.md) |
| Settings (general, security, appearance, widgets, notifications, AI, monitoring, system — 8 rail groups) | [guides/settings.md](./guides/settings.md) |
| Themes | [guides/themes.md](./guides/themes.md) |
| Desktop shell & windowing | [guides/desktop.md](./guides/desktop.md) |
| Activity taskbar (unified background-job feed) | [guides/activity.md](./guides/activity.md) |
| Apps Center | [guides/apps-center.md](./guides/apps-center.md) |
| Server control (restart/shutdown) | [guides/server-control.md](./guides/server-control.md) |
| Health monitoring | [guides/health.md](./guides/health.md) |
| Notifications (persistent center, health/backup alerts, email, webhook, What's New) | [guides/notifications.md](./guides/notifications.md) |
| Antivirus (ClamAV) | [guides/antivirus.md](./guides/antivirus.md) |
| Anti-Ransomware (honeypots, rate detection) | [guides/ransomware.md](./guides/ransomware.md) |
| Firewall (ufw) | [guides/firewall.md](./guides/firewall.md) |
| HTTPS / TLS (built-in listener, self-signed + manual certs) | [guides/tls.md](./guides/tls.md) |
| System Journal (log viewer, auto-cleanup) | [guides/journal.md](./guides/journal.md) |
| Widgets (desktop monitor catalog) | [guides/widgets.md](./guides/widgets.md) |
| Web shortcuts (pinned desktop links, SSRF-guarded metadata fetch) | [guides/shortcuts.md](./guides/shortcuts.md) |
| Design system (single UI identity — tokens, components, page anatomy) | [guides/design-system.md](./guides/design-system.md) |
| Development workflow (multi-instance `.local`, real-VM tests, client contract) | [guides/development.md](./guides/development.md) |

## Doc conventions

- Files use `kebab-case.md`.
- Each doc: one-line purpose, overview, relevant endpoints/components with
  `path:line` references, request/response shapes, edge cases/invariants, and
  links to related docs.
- Endpoint paths are relative to the SPA base `/api` (e.g. `/api/auth/login` is
  written as `auth/login`).
