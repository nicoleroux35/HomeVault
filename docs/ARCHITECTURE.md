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
`cargo build --release`. This covers the pre-compressed `.br` / `.gz` siblings
too — they are written by `npm run build` (npm `postbuild`) *after* the last
`cargo build` may have embedded `static/`, so a non-clean release build can
silently ship uncompressed assets. `installer/install.sh --package` and
`compile_backend_release.sh` build from clean, so releases are unaffected.

`npm run build` also runs `prebuild` (`generate-sprite.mjs`, regenerating the
icon sprite from the sources) — see
[Frontend Structure](./FRONTEND.md) → Icon sprite.

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
- Versioned client/server contract: `middleware::contract` rejects an
  explicit `X-HomeVault-Contract` mismatch on `/api/*` with **426 Upgrade
  Required** (`/api/meta` exempt); the SPA sends the header on every request
  and shows a reload banner on a 426. See "Versioned client/server contract"
  below.
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

### Asset caching, validators and pre-compression (`spa.rs`)

`spa::build_assets()` walks the embedded tree once (lazily) into a
`path → Asset` table, so a request costs one hash lookup. Two things are
derived per file and matter for correctness:

- **`ETag`** — a truncated SHA-256 of the *identity* bytes, with a `-br` /
  `-gz` suffix per content coding. Without a validator a browser can never
  issue `If-None-Match`, so before this change `/wallpapers/*.webp` (several
  MB) and the icon sprite were re-downloaded on every visit. `spa` now answers
  a matching `If-None-Match` with `304` and no body. Keep the per-coding ETags
  distinct: a cache holding the identity body must not validate a `br`
  request.
- **`Cache-Control`**, chosen per route (there is deliberately **no** blanket
  layer in `create_app` — one applied after the merges would also pin API
  responses and `index.html` for a year):
  - `CACHE_IMMUTABLE` (`public, max-age=31536000, immutable`) for
    `/assets/*`, `/fonts/*` (content-hashed by Vite), `/wallpapers/*` and
    `/static/*` (release-scoped, hand-authored).
  - `CACHE_REVALIDATE` (`public, max-age=86400`) for `/sprite.svg` and
    `/sprite-unified.svg`: their URL is fixed but their content is regenerated
    from the sources on every build, so `immutable` would pin a stale sprite
    (blank new icons) in every browser.
  - none for `serve_dist_file` (`/manifest.json`, `/favicon.svg`, `/sw.js`) and
    none for `serve_spa` (`index.html`): a stale `index.html` or `sw.js` would
    strand an upgraded client on the previous release.

**Pre-compressed variants.** `frontend/scripts/precompress-dist.mjs` (npm
`postbuild`, so it runs after every `vite build`) writes a `.br` and a `.gz`
sibling next to each compressible file under `static/dist`. The same
`include_dir!` embeds them, and `spa` serves them when the client's
`Accept-Encoding` allows, setting `Content-Encoding` + `Vary: accept-encoding`.
The `Content-Encoding` header is also what makes
`tower_http::compression::CompressionLayer` skip the response — it never
re-compresses an already-encoded body — so the compressor is now only a
fallback for files that were too small to bother with. Compressed siblings are
never addressable directly (a `.br` path 404s).

## Deployment model (runs as root)

Since commit `2d5ba40` (2026-09-04) there is **no dedicated `homevault` service
account**. The systemd unit `installer/install.sh` writes has no `User=`/`Group=` so systemd runs
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

### Identity & first-run state (2026-10-13)

The administrator accounts and the first-run setup state both live in the
SQLite database at `{DATA_DIRECTORY}/data/homevault.db` (default
`/var/lib/homevault/data`): `users` answers "does an admin exist?" and the
singleton `setup_state` row (migration 037) holds the hardware fingerprint
recorded at setup. There are **no** `.setup_complete`/`.system_hash` marker
files any more. Pointing `DATA_DIRECTORY` at a persistent volume therefore
makes the whole identity survive a reinstall of the system disk. See
[Setup Wizard](./guides/setup-wizard.md).

## Installer — `installer/install.sh`

A **single self-contained file** — the only installer in the repo. Packages,
the systemd unit, the udiskie automount config, the download and the final
verification all live in it; nothing is read from a sibling file (the retired
`homevault.service`, `setup-automount.sh`, `optional-packages.conf` and
packaging `Makefile` were folded in). Meant to be piped onto a fresh machine:

```bash
curl -fsSL https://raw.githubusercontent.com/nicoleroux35/HomeVault/main/installer/install.sh | sudo bash
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
3. **Application** — downloads the pre-built binary from the project's GitHub
   Releases, as the per-architecture asset `homevault-<arch>.tar.gz`
   (`HV_RELEASE_URL` overrides the URL, `HV_RELEASE_SHA256` optionally pins the
   checksum, `$1` uses an already-downloaded file), extracts it to
   `/opt/homevault`, writes the systemd unit and the udiskie automount, and
   optionally enables HTTPS.
4. **Verification** — re-checks the packages, the service, the HTTP endpoint on
   `PORT` and that the binary really landed in `/opt/homevault`. For Ollama it
   goes further than presence: the `ollama` unit must be active and
   `http://127.0.0.1:11434/` must answer, since the Chat app needs the
   daemon's API, not just the binary.
5. **Done** — English summary plus the LAN address and the public address
   (the latter only works with port forwarding).

Maintainers produce the release assets with the same file, from a checkout
(needs Rust + Node ≥ 20):

```bash
./installer/install.sh --package   # installer/release/homevault-<arch>.tar.gz (+ .sha256)
```

The archive is a single `homevault` binary that already contains the built UI,
so both frontend build steps run before `cargo build` and are part of the
release: `npm run build` regenerates the **subset** icon sprite (`prebuild`,
see [FRONTEND.md](./FRONTEND.md) → Icon sprite) and writes the pre-compressed
`.br` / `.gz` siblings (`postbuild`, see the SPA-serving section above).
Skipping or reordering that would ship a full-sprite, on-the-fly-compressed
binary.

`src/dependencies.rs`'s tests keep the two sides honest: every package in the
runtime inventory must appear in `install.sh`, the inline systemd unit and
udiskie config must keep their load-bearing directives, and `install.sh` is
asserted to stay free of any reference to a sibling installer file.

## Versioned client/server contract

The backend and the Svelte SPA are built and released together, but a browser
can hold a cached bundle from the previous build after an update (service
worker, aggressive proxy cache, a long-lived tab). A versioned contract makes
that fail loudly instead of silently misparsing.

- `src/api_contract.rs` — `CONTRACT_VERSION` is bumped **only** on a breaking
  API change (renamed field, removed endpoint, changed shape); additive
  changes do not bump it. `client_is_compatible` treats a missing or
  non-numeric header as legacy (allowed) and a present, different value as
  incompatible.
- `GET /api/meta` (`src/handlers/meta.rs`, unauthenticated) reports
  `{ contract_version, server_version }` — the discovery endpoint the SPA can
  call before it has a session.
- `src/middleware/contract.rs` rejects an incompatible request on `/api/*`
  with `426 Upgrade Required`.
- `frontend/src/lib/services/api.ts` defines the matching
  `CONTRACT_VERSION`, sends `X-HomeVault-Contract` on every request, and flags
  a 426 into `stores/contract.ts`; `Desktop.svelte` renders the reload banner
  (`[data-testid="contract-mismatch-banner"]`).

**Rule:** bump `CONTRACT_VERSION` in **both** `src/api_contract.rs` and
`frontend/src/lib/services/api.ts` in the same change, and only for a breaking
change.

## Auth model (summary)

Two guard helpers in `src/handlers/admin_guard.rs`:

- `require_auth(state, headers)` → any valid session (user or admin).
- `require_admin(state, headers)` → session must belong to an admin.

Both read `X-Session-Token`, look up the username via
`auth.get_username_from_token`, and verify admin via `auth.is_admin`. Most
dashboard/network/storage endpoints that previously had no guard now use
`require_auth`; mutations use `require_admin`. See
[API Contract](./API.md) for the per-endpoint level.

### Per-app proxy auth (second entry)

A browser navigating to a proxied app's port sends no `X-Session-Token`, only
cookies. Login/2FA also set an `hv_session` cookie for that flow, but the app
proxy does **not** trust it: it accepts only an app-proxy-scoped
`hv_app_session` token minted by `GET /app-auth?app=<ns_id>` on the main
origin (`src/handlers/app_auth.rs`, `src/app_proxy/app_auth.rs`). `/app-auth`
validates the real session, mints the scoped token and redirects to the app
origin; the proxy then strips both HomeVault cookies before forwarding
upstream. The scoped token grants proxy access only and is revoked on logout
or when the proxy stops. See [guides/docker.md](./guides/docker.md).

## Related docs

- [Backend Structure](./BACKEND.md)
- [Frontend Structure](./FRONTEND.md)
- [API Contract](./API.md)
