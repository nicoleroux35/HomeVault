# Backend Structure

Rust + Axum. Entrypoint: `src/bin/server.rs` → `src/web/mod.rs::run()`.
Source layout (`src/`):

```
src/
  web/mod.rs            Router assembly + server startup (create_app, run)
  handlers/mod.rs       AppState definition (shared services)
  handlers/             One module per domain (auth, users, user_homes, groups,
                        dashboard, storage, encryption, shares,
                        permissions, network, chat, rag, restic, docker, compose,
                        docker_catalog, rclone, syncthing, downloader, journal,
                        transfer_quotas, security, settings, avatars,
                        ups, snmp, monitor_layout, weather_location, terminal_pty,
                        rescue, antivirus, firewall, ransomware, cache,
                        notifications, progress, system, share_links,
                        users_models, widgets, …) — `handlers/quotas.rs` and
                        `handlers/snapshots.rs` were removed (see
                        [transfer-quotas](./guides/transfer-quotas.md) and
                        [Snapshots](./guides/snapshots.md)).
  storage/              SQLx repositories (user_repo, group_repo, user_home_repo,
                        restic_repo, transfer_quota_repo, sync_job_repo,
                        share_link_repo,
                        journal_cleanup_repo, favorite_repo, app_store_repo, …)
  auth/                 AuthService, session creation, 2FA/TOTP, middleware
  config/               Settings, paths, PathGuard
  app_catalog/          Manifest-driven Docker app catalog + registry digest
                        update detection (replaces the deleted docker_catalog.rs)
  app_proxy/            Per-installed-app reverse proxy (47000-47999); gate
                        accepts only the app-scoped hv_app_session minted by
                        GET /app-auth, and strips both HomeVault cookies before
                        forwarding upstream (app_auth.rs, cookie.rs, serve.rs)
  rescue/               ddrescue / TestDisk / PhotoRec / mdadm-repair service +
                        SecurityValidator (protected-device blocklist)
  groups.rs             GroupService (incl. is_admin_group)
  disk.rs               DiskService (disks, partitions, mdadm RAID create/delete)
  pools.rs              PoolService (ZFS / Btrfs pools, scrub, scan_pools)
  cache.rs              Disk cache tier (mode, mover, cached shares)
  encryption.rs         LUKS2 encrypt/unlock/lock for standalone devices
  acl.rs                setfacl/getfacl wrappers (sudo)
  sftp.rs               SftpJailService — real second sshd, per-user chroot jails
  quota.rs              Per-user home-directory disk limit (setquota -u).
                        The per-group setquota -g feature was removed 2026-09-09.
  transfer_quota.rs     Per-user/group upload/download caps (Explorer only)
  autostart.rs          Start-on-boot (systemd) state for `homevault.service`
  avatars.rs            Offline avatar generation (local PRNG, no API calls)
  compose.rs            Docker Compose project management
  docker.rs             Docker daemon wrapper
  logging.rs            Rotating file log appender
  media_ticket.rs       Short-lived media access tickets
  setup.rs              First-run setup-wizard state machine
  notification_engine.rs  Background loop: health/backup/app-update -> notifications
  restic.rs / restic_scheduler.rs   Restic wrapper + cron backup scheduler loop
  antivirus.rs / antivirus_scheduler.rs   ClamAV scan + scheduled scans
  ransomware.rs / ransomware_watcher.rs    Honeypot + rate-based detection
  firewall.rs           ufw wrapper (locale-independent via ufw_command())
  events.rs             log_event / log_event_warn — the application audit trail
                        (feeds the System Journal "app" source via tracing)
  dependencies.rs       Startup external-tool inventory (cross-checked vs install.sh)
  email.rs              EmailService (mutt/sendmail/msmtp)
  permissions.rs        PermissionService (POSIX ACLs for shares)
  shares.rs             Share model/service (SMB/NFS/FTP/WebDAV/SFTP)
  browser.rs            FileBrowser (Explorer: list/copy/move/trash, collisions)
  snapshots.rs          ZFS/Btrfs snapshot create/list/rollback
  network.rs vpn.rs …   Domain services
  incus.rs              Incus REST client over its Unix socket (VMs + system
                        containers): list/create/lifecycle/snapshots, operation
                        waiting, OS catalogue, cloud-init builder, console WS
  mdns.rs               Avahi publishing — homevault.local + share discovery
                        (auto reconciler, spawned from web/mod.rs)
  settings_persistence.rs  SettingsPersistence (export/import; redacts smtp_password)
  i18n.rs               Translation engine, embeds src/i18n/*.toml at compile time
  middleware/           csrf, rate_limit, fail2ban, security_headers, logging
  sse/                  Server-sent-event progress stream (ProgressManager: last
                        event retained + owner, feeds GET /api/activity)
  health.rs             HealthService
  rag.rs                RAG core: chunking, Ollama embeddings, cosine similarity, search
  rag_indexers.rs       Document indexers (catalog, features, i18n) for RAG knowledge base
```

## AppState (`src/handlers/mod.rs`)

Shared state (`AppStateArc = Arc<AppState>`) passed to every handler:

- `auth: AuthService` — sessions, users, 2FA.
- `widgets: WidgetRepository`.
- `db: Database` (wraps `SqlitePool`).
- `path_guard: PathGuard` — restricts file operations to allowed directories.
- `admin_rate_limiter: Arc<RateLimitState>` — 10 ops/min per admin.
- `pty_manager: Arc<PtyManager>` — native terminal sessions.
- `downloader: handlers::downloader::Aria2State` — aria2 daemon handle.
- `ransomware_watch_tx: mpsc::UnboundedSender<ransomware_watcher::WatchCommand>`
  — reconfigures the running ransomware watcher (protected-path add/remove)
  without a restart.

## Handlers

Each `handlers/<domain>.rs` exposes `async fn handle_*(State(state),
...)` functions wired to routes in `web/mod.rs`. Handlers return
`Json<serde_json::Value>` (or `ApiResponse`). Auth is enforced inline by calling
`require_auth` / `require_admin` (`src/handlers/admin_guard.rs`) at the top of
mutating/reading handlers.

Notable handlers (see `API.md` for the full map):

- `handlers/auth.rs` — login, 2FA verify/setup, password reset, setup, current user.
- `handlers/users.rs`, `handlers/groups.rs` — account & group management.
- `handlers/dashboard.rs` — system metrics + widget CRUD.
- `handlers/storage.rs` — disks/partitions/RAID/virtual-disks/SMART (also serves RAID handlers).
- `handlers/shares.rs`, `handlers/permissions.rs` — file shares + ACLs.
- `handlers/network.rs` (`create_network_router`), `handlers/vpn.rs`
  (`create_vpn_router`), `handlers/rescue.rs` (`create_rescue_router`),
  `handlers/terminal_pty.rs` (`create_terminal_router`),
  `handlers/vm.rs` (`create_vm_router` — just the console WebSocket; the REST
  routes are registered directly) — nested sub-routers.
- `handlers/vm.rs` — Virtual Machines app (Incus). Admin-only. Curated OS
  wizard, size presets, cloud-init-seeded login, tracked `vm_create` job,
  operation-waiting lifecycle, and an xterm shell console relayed over
  `tokio-tungstenite`. See [guides/vm.md](./guides/vm.md).
- `handlers/restic.rs` — backup repos, snapshots, schedules, retention.
- `handlers/chat.rs` — Ollama-backed AI assistant.
- `handlers/activity.rs` — `GET /api/activity`, the unified background-job feed
  for the desktop taskbar (merges `PROGRESS_MANAGER` jobs + aria2 downloads).
  See [guides/activity.md](./guides/activity.md).
- `handlers/rag.rs` — RAG knowledge base (status, index, search, documents).
- `rag.rs` — core RAG module: chunking, Ollama embeddings, cosine similarity, SQLite storage.
- `rag_indexers.rs` — document indexers for catalog, features, and i18n sources.
- `handlers/docker.rs`, `handlers/compose.rs`, `handlers/docker_catalog.rs`
  (manifest-driven; the app packages live in `catalog/`, the logic in
  `src/app_catalog/*`, the per-app proxy in `src/app_proxy/*`),
   `handlers/rclone.rs`, `handlers/syncthing.rs`, `handlers/downloader.rs`,
   `handlers/security.rs`,
  `handlers/settings.rs`, `handlers/avatars.rs`, `handlers/ups.rs`,
  `handlers/snmp.rs`. (`handlers/wallpapers.rs` was removed 2026-09-04 — the
  15 bundled wallpapers are static assets, not an API.)
- `handlers/user_homes.rs` — per-user home folder: create/enable/disable/delete,
  disk choice, and the **home-directory size limit** (`setquota -u`, best-effort,
  see [guides/users.md](./guides/users.md) and [guides/quotas.md](./guides/quotas.md)).
- `handlers/encryption.rs` — standalone LUKS2 volumes (list/create/unlock/lock/delete).
- `handlers/notifications.rs` — list/unread-count/mark-read/mark-all-read/delete,
  plus the SSE ticket/stream pair (`handlers/notifications.rs`'s
  `handle_notifications_events_ticket`/`handle_notifications_events`).
- `handlers/antivirus.rs`, `handlers/firewall.rs`, `handlers/ransomware.rs` —
  the security suite (ClamAV, ufw, honeypot/rate-based ransomware detection).
- `handlers/tls.rs` + `tls.rs` — built-in HTTPS: `GET/PUT /api/settings/tls`,
  `POST /api/settings/tls/regenerate`. `tls.rs` owns self-signed generation
  (`rcgen`) + renewal, cert introspection (`x509-parser`), and the optional
  second `axum_server` listener wired in `web/mod.rs::run()`. Opt-in, no DB.
  See [guides/tls.md](./guides/tls.md).
- `handlers/cache.rs` — disk cache tier (pools, cache shares, mover runs).
- `handlers/journal.rs` — centralized log viewer (app log, journald, text logs)
  plus its auto-cleanup schedule.
- `handlers/transfer_quotas.rs` — per-user/group upload/download caps,
  enforced only against File Explorer transfers (distinct from the per-user
  home-dir `setquota -u` limit in `quota.rs`).
- `handlers/monitor_layout.rs`, `handlers/weather_location.rs` — per-user
  desktop-monitor position/visibility and the Weather monitor's saved city.

## Storage / repository layer

`src/storage/` holds SQLx repositories. Each repo wraps a pool and provides typed
async CRUD. Migrations are in `sqlx/migrations/` (**001–038**), embedded in the
binary via `include_dir!` and applied at startup. `storage/database.rs::Database::run_migrations`
runs each pending file once (tracked in `_hv_migrations`); the hand-rolled SQL
statement splitter tracks `--` line comments explicitly, so a semicolon inside a
comment doesn't get treated as a statement terminator; `migrate_roles_005` seeds
default roles. Recent migrations: `027_app_stores.sql`,
`028_password_recovery_codes.sql`, `029_drop_password_reset_tokens.sql`
(the old email reset flow was deleted), `030_drop_user_wallpapers.sql`,
`031_explorer_favorites.sql`, `032_user_home_size.sql` (`user_homes.size_limit_bytes`),
`033_rag_knowledge_base.sql` (RAG documents, chunks, index status),
`034_explorer_share_links.sql` (Explorer "share by link"),
`035_login_attempts_success_user_agent.sql` (user-agent on successful logins),
`036_sync_jobs.sql` (background sync job tracking),
`037_setup_state.sql` (durable first-run setup state — see below).

All repositories share a common `Repository` trait (`src/storage/repository.rs`)
exposing standard CRUD (`find_by_id`, `find_all`, `create`, `update`, `delete`,
`exists`, …). The `impl_repository!` macro (`repository.rs`) generates the boilerplate
`Clone` + `Repository` impl for a struct that holds an `Arc<SqlitePool>`:

```rust
#[derive(Clone)]
pub struct UserRepository { pool: Arc<SqlitePool> }
impl_repository!(UserRepository);
```

This keeps every repo on a standardized interface and avoids code duplication. Some
repos add pagination (`find_all_paginated(limit, offset)`, `count`), bulk deletes
(`delete_many(ids)`), and domain lookups (e.g. `UserRepository::find_by_email`,
`SessionRepository::find_all_by_username`, `User2faRepository::is_enabled`).

Multi-table operations that must be atomic live in `TransactionalService`
(`src/storage/transactions.rs`), which opens a single `sqlx` transaction per call:

- `create_user_with_history` — insert user + first password-history row.
- `update_password_with_history` — archive old hash, set new one.
- `delete_user_cascade` / `delete_users_cascade_batch` — delete user and all
  related rows (sessions, widgets, password history, login attempts, 2FA).
- `update_user_role_with_sessions` — change role and refresh the cached admin flag
  on active sessions.
- `lock_user_and_clear_sessions` — lock account and force-logout.
- `create_group_with_quota` / `delete_group_cascade` — group + disk quota (system
  groups protected).
- `cleanup_old_data(days_sessions, days_attempts)` — purge expired sessions and old
  login attempts.

SQLite runs in WAL mode with a tuned cache/MMAP and foreign keys enforced; indexes
for common queries are created by the migrations.

### First-run setup state (`storage/setup_state_repo.rs`)

The database — not a marker file — is the source of truth for whether the
appliance is configured. `SetupStateRepository::has_admin()` (a `users` row with
`role_id = 2`) is the canonical check used by the setup handlers, and the
singleton `setup_state` row (migration 037) carries the `system_hash` /
`machine_id` fingerprint recorded at setup time for hardware-change detection.
See [Setup Wizard](./guides/setup-wizard.md).

## Configuration (`src/config/`)

- `Settings::load()` reads `Settings` (server name, port, timezone, data dir,
  email, etc.).
- `paths.rs` — `data_dir()`, `database_path()`, `logs_dir()`, `quarantine_dir()`,
  `sftp_jails_dir()`/`sftp_keys_dir()`, `apps_dir()`, etc. (`wallpapers_dir()`
  was removed with the upload-wallpaper feature.)
- `PathGuard` — validates that file paths used by Explorer/shares stay inside
  allowed roots.

See **Deployment model** in [Architecture](./ARCHITECTURE.md): the process runs
as root, no `homevault` service account, no generated sudoers file —
privileged shell-outs stay `sudo`-wrapped, except `quota.rs`'s bare
`setquota` (deliberate: the process runs as root, documented in
`src/quota.rs`).

## Auth & sessions (`src/auth/`)

- `AuthService` creates sessions (`create_session`), validates tokens
  (`validate_session_token`), maps token→username (`get_username_from_token`),
  and checks admin (`is_admin`).
- 2FA/TOTP: `generate_totp`, `encode_base32`/`decode_base32` (base32 secrets must
  round-trip correctly).
- **Account recovery**: the email/token "forgot password" flow was **deleted**
  (2026-09-03). It is replaced by a single 32-digit **offline recovery code**
  shown once at the end of the Setup Wizard; only its Argon2 hash is stored
  (`password_recovery_codes`, `src/storage/password_recovery_repo.rs`).
  `generate_recovery_code` / `verify_recovery_code` (rate-limited under a
  `recovery:` key, separate from the login lockout) / `reset_password_with_recovery_code`
  (does **not** consume the code). See [guides/auth.md](./guides/auth.md).
- `middleware.rs::auth_middleware` is an Axum middleware variant (returns 401),
  but most endpoints use the `require_auth`/`require_admin` helper guards instead.
- Login / 2FA / setup / logout also set a `hv_session` **cookie** — read only
  by `GET /app-auth` (`src/handlers/app_auth.rs`), which mints the
  app-proxy-scoped `hv_app_session` the proxy actually trusts; the JSON API
  still reads only the `x-session-token` header. See
  [guides/docker.md](./guides/docker.md).
- `api_contract.rs` + `handlers/meta.rs` + `middleware/contract.rs` implement
  the versioned client/server contract (`GET /api/meta`, `X-HomeVault-Contract`,
  426 on mismatch). `release_notes.rs` holds the curated What's New highlights.

## i18n (`src/i18n.rs`)

Loads the embedded `src/i18n/{en,fr,es,de}.toml` content (compiled in with
`include_str!` — no data files needed at runtime). `init_i18n(None)` is called
in `run()` (`src/web/mod.rs`, grep for `init_i18n`). Frontend fetches the active map via
`GET /translations` (mounted at `/api/translations` in
`src/web/mod.rs`, **not** `/api/settings/translations`). See [i18n](./I18N.md).

## Background tasks

Background loops spawned in `run()` (see [Architecture](./ARCHITECTURE.md) for
the full list; line numbers drift, grep for `tokio::spawn`):
`ransomware_watcher::run_watcher_loop`,
`restic_scheduler::run_scheduler_loop` (computes next run via
`compute_next_run`, marks jobs `completed`/`failed`), `cache_scheduler`,
`antivirus_scheduler`, `journal_cleanup_scheduler`, and
`notification_engine::run_notification_loop` (polls every 60s, turning
health-check status transitions — `HealthService::run_all_checks`, in-memory
dedup so the same problem doesn't re-notify every tick — and new restic
backup job results into `notifications` rows, optionally dispatched via
email/webhook). Plus terminal cleanup on shutdown (`shutdown_signal` →
`pty_manager.cleanup_all`).

## Running & checking

```bash
cargo check                 # type-check (run after any change)
cargo build --release
cargo run --bin homevault
```

## Related docs

- [Architecture](./ARCHITECTURE.md)
- [API Contract](./API.md)
- [Frontend Structure](./FRONTEND.md)
