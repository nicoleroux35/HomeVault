# Architecture Overview

HomeVault is a single Axum HTTP server that serves both a JSON API (under `/api`)
and the compiled Svelte SPA (static files + SPA fallback to `index.html`).
There is no separate API process.

## Components

- **Backend**: Rust, Axum web framework, SQLx with an embedded SQLite database.
  Source in `src/`.
- **Frontend**: Svelte 5 (runes) + TypeScript, Vite build, output to `static/dist`.
  Source in `frontend/src/`.
- **Database**: SQLite file at `data/homevault.db` (path from
  `config::paths::database_path()`). Migrations in `sqlx/migrations/` are applied
  automatically at startup (`Database::run_migrations` in
  `src/storage/database.rs`, `pub async fn run_migrations`).

## Request flow

```
Browser (Svelte SPA)
  → fetch('/api/...', { headers: { 'X-Session-Token', 'X-CSRF-Token' } })
  → Axum Router (src/web/mod.rs create_app)
       → handler (src/handlers/*)
       → storage/repo layer (src/storage/*) or service (src/*.rs)
       → SQLite (sqlx pool in AppState)
  ← JSON { success, data | error }
```

## Build & run

### Backend

```bash
# from project root
cargo build                 # debug build
cargo run --bin homevault   # binary defined in Cargo.toml [[bin]], src/bin/server.rs
cargo build --release
```

The server reads configuration from `config::Settings::load()` (see
[Backend Structure](./BACKEND.md) → configuration). The listening port comes from
`settings.port` and defaults to `0.0.0.0:<port>` (`src/web/mod.rs`, `run()` —
grep for `0.0.0.0:`).

Required OS tooling (used by various handlers, not needed to boot):
`restic`, `mdadm`, `rclone`, `aria2c`, `docker`, `syncthing`, `ddrescue`,
`testdisk`, `photorec`, `smartctl`/`lsblk`.

### Frontend

```bash
cd frontend
npm install
npm run dev        # Vite dev server (HMR), proxies /api to backend
npm run build      # production build into static/dist
npm run check      # svelte-check (type checking)
```

**Build gotcha — the frontend and i18n are baked into the binary.**
`src/handlers/spa.rs` does `include_dir!("static")` and `src/i18n.rs` does
`include_str!("i18n/*.toml")`. Neither macro emits a `cargo:rerun-if-changed`,
so after a frontend-only rebuild (`npm run build`) or a `.toml` edit, a plain
`cargo build` is a no-op and the stale bundle stays embedded. Force it:
`touch src/handlers/spa.rs` (frontend) / `touch src/i18n.rs` (i18n) before
`cargo build --release`.

### First run

1. Start the backend (`cargo run --bin homevault`).
2. Open the app in a browser. On first run the **Setup Wizard** is shown
   (backend reports `is_first_run` from `GET /api/auth/setup-status`).
3. Create the administrator account; subsequent logins use the Login page.

## Background tasks

Loops spawned in `run()` (`src/web/mod.rs`; line numbers drift, grep for
`tokio::spawn`):

- **Ransomware watcher** (`ransomware_watcher::run_watcher_loop`) — honeypot/
  mass-file-op detection, debounced per protected path; reconfigured live via
  `ransomware_watch_tx`.
- **Restic scheduler** (`restic_scheduler::run_scheduler_loop`) — picks up due
  backup schedules, computes next run via `compute_next_run`; a failed run still
  reschedules and marks the job `failed`.
- **Cache mover scheduler** (`cache_scheduler::run_scheduler_loop`).
- **Antivirus scheduler** (`antivirus_scheduler::run_scheduler_loop`).
- **Journal cleanup scheduler** (`journal_cleanup_scheduler`) — day-interval
  event-log retention; default **on / 90 days** (privacy by default). The first
  tick fires immediately: it must only *persist a baseline*, never clean, when
  there is no prior `last_cleanup_at` row.
- **Notification engine** (`notification_engine::run_notification_loop`) —
  polls every 60s, turns health-check transitions and restic job results into
  `notifications` rows (in-memory dedup), optionally emailed/webhooked; also runs
  the every-6h Docker-app-update check (`check_app_updates`).

**Terminal PTY cleanup**: on graceful shutdown the `shutdown_signal` handler
calls `pty_manager.cleanup_all()` so shells never outlive the server
(`src/web/mod.rs`, `shutdown_signal` at the bottom of the file).

## Middleware / cross-cutting

- CORS: permissive `Any` (dev convenience) in `src/web/mod.rs` (grep for
  `CorsLayer::new`).
- `RequestBodyLimitLayer`: 10 GiB (`src/web/mod.rs`, grep for
  `RequestBodyLimitLayer::new`).
- CSRF protection: `middleware::csrf` — the SPA sends `X-CSRF-Token`.
- Rate limiting: admin rate limiter 10 ops/min.
- `fail2ban` and `security_headers` middleware in `src/middleware/`. HSTS is
  emitted only when the built-in HTTPS listener is active
  (`crate::tls::tls_active()`).
- `middleware::https_redirect` — added to the **HTTP** router only when the
  admin has enabled TLS + the redirect toggle; 301s non-loopback plain-HTTP
  requests to HTTPS.

## TLS / HTTPS

The server is plain HTTP on `PORT` (8080) by default. Built-in HTTPS is
**opt-in and additive** (`TLS_ENABLED` / Settings > Security): `web::run()`
also binds an `axum_server` TLS listener on `HTTPS_PORT` (8443) serving the
same router, with a HomeVault-generated self-signed certificate or an
admin-supplied PEM pair. Applies at startup (restart to change). TLS failure is
logged, never fatal. Fronting HomeVault with an external reverse proxy that
terminates TLS is still supported. Full detail: [guides/tls.md](./guides/tls.md).

## SPA serving

`create_app` merges a `spa` router that serves `/sprite.svg`, `/manifest.json`,
favicon, and a `fallback` to `serve_spa`. Static assets come from the
`include_dir!`-embedded `static/` tree (`spa::STATIC_DIR`). Wallpapers are
**bundled** — `frontend/public/wallpapers/hv-NN.webp` are copied into
`static/dist/` by `vite build` and served by `spa::serve_wallpapers_file` on
`GET /wallpapers/{*path}`, which **must stay registered before** the `serve_spa`
fallback or images resolve to `index.html`. The old upload-your-own-wallpaper
feature (`handlers/wallpapers.rs`, `/api/wallpapers*`, `config::paths::wallpapers_dir()`,
the `user_wallpapers` table) was removed 2026-09-04 — see
[guides/wallpapers.md](./guides/wallpapers.md).

## Deployment model (runs as root)

Since commit `2d5ba40` (2026-09-04) there is **no dedicated `homevault` service
account**. `installer/homevault.service` has no `User=`/`Group=` so systemd runs
it as **root**; `installer/install.sh` only creates `/home/homevault` as a
home-folder root and the first admin's real Linux account at setup. Consequences:

- `SetupService::configure_sudoers()` is **dead code** — no sudoers file is
  generated. Every "the sudoers grant must not be removed" note in older docs is
  historical.
- The `sudo` wrappers on the individual commands (`mdadm`, `zfs`/`zpool`,
  `cryptsetup`, `useradd`/`groupadd`, `setquota`, `ufw`, `journalctl`, `mount`,
  `setfacl`, …) **stay** — they succeed against the distro-default
  `root ALL=(ALL:ALL) ALL` and keep a future non-root mode possible.
- On a dev box the binary is typically run directly as an unprivileged user
  (e.g. `nicolas`), so `sudo`-needing features WARN at startup and degrade
  gracefully; that is expected in development.

## Installer — `installer/install.sh`

One entry point, meant to be piped onto a fresh machine:

```bash
curl -fsSL https://codeberg.org/nicleroux35/HomeVault-Svelt/raw/branch/main/installer/install.sh | sudo bash
```

Five steps, in order (`STEP_TOTAL` in the script):

1. **Pre-flight** — root, supported distro (Debian 10+ / Ubuntu 20.04+), CPU
   architecture (`x86_64` / `aarch64` / `armv7l`), free disk space.
2. **System packages** — creates the directory tree, then installs the required
   and optional package groups (inventory cross-checked against
   `src/dependencies.rs`); optional groups degrade per package instead of
   failing as a whole. Ollama (the AI assistant's runtime) is not an apt
   package: it is installed best-effort through its own official script, with
   no model downloaded.
3. **Application** — downloads the release archive (`HV_RELEASE_URL`, or `$1`
   for an already-downloaded file), extracts it to `/opt/homevault`, installs
   the systemd unit and the udiskie automount, optionally enables HTTPS.
4. **Verification** — re-checks the packages, the service, the HTTP endpoint on
   `PORT` and that the binary really landed in `/opt/homevault`. For Ollama it
   goes further than presence: the `ollama` unit must be active and
   `http://127.0.0.1:11434/` must answer, since the Chat app needs the
   daemon's API, not just the binary.
5. **Done** — English summary plus the LAN address and the public address
   (the latter only works with port forwarding).

Because a piped run has no checkout on disk, `install.sh` carries embedded
copies of `installer/homevault.service` and of the udiskie automount config;
`src/dependencies.rs`'s tests keep those copies identical to the tracked files.
A checkout install (`sudo bash installer/install.sh`) copies/runs the tracked
files verbatim instead.

## Auth model (summary)

Two guard helpers in `src/handlers/admin_guard.rs`:

- `require_auth(state, headers)` → any valid session (user or admin).
- `require_admin(state, headers)` → session must belong to an admin.

Both read `X-Session-Token`, look up the username via
`auth.get_username_from_token`, and verify admin via `auth.is_admin`. Most
dashboard/network/storage endpoints that previously had no guard now use
`require_auth`; mutations use `require_admin`. See
[API Contract](./API.md) for the per-endpoint level.

## Related docs

- [Backend Structure](./BACKEND.md)
- [Frontend Structure](./FRONTEND.md)
- [API Contract](./API.md)
