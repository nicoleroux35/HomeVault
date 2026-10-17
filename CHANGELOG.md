# Changelog

All notable changes to HomeVault are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the versioning
follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **Scheduled Tasks** (new app, migration `043_scheduled_tasks.sql`): one
  generic cron engine replacing per-feature schedulers for admin-defined
  recurring jobs — single-executable commands (no shell, absolute path,
  30-minute timeout), pool snapshots, S.M.A.R.T. self-tests and host power
  actions. Payloads are validated at creation *and* at run time
  (`TaskAction::from_parts`); every execution is recorded in a run history
  with an output tail. See `docs/guides/scheduled-tasks.md`.
- **Metrics** (new app, migration `044_metrics_history.sql`): a 60 s
  collector (CPU deltas, memory, load, network throughput, max thermal-zone
  temperature) with 8-day retention, bucketed 1 h / 24 h / 7 d SVG charts,
  gaps rendered as gaps. See `docs/guides/metrics.md`.
- **Host power management + disk spindown** (Settings → System, migration
  `045_disk_power.sql`): CPU governor (validated against the kernel list),
  host reboot/shutdown/suspend, and per-disk hdparm APM + idle timeout that
  are re-applied at every startup. See `docs/guides/power.md`.
- **Per-event notification toggles + webhook sink formats**: every engine
  notification carries a stable event id (`backup.jobs`, `app.updates`,
  `health.<check>`); admins mute events individually
  (`GET|PUT /api/notifications/events/config`, absent = enabled) and choose
  the webhook payload shape (`generic` | Slack | Discord). See
  `docs/guides/notifications.md`.
- **Snapshot schedules with GFS retention** (Storage → new "Snapshots" tab,
  migration `046_snapshot_schedules.sql`): cron-driven `{prefix}-<timestamp>`
  snapshots per pool with grandfather-father-son pruning (newest N
  hourly/daily/weekly/monthly/yearly generations); pruning only ever deletes
  snapshots carrying the schedule's own prefix — manual and backup snapshots
  are untouchable. See `docs/guides/snapshots.md`.
- Fixed a latent `api.ts` bug where `ttl: 0` is falsy and silently fell back
  to the 5 s default TTL (the topbar notification list, What's New, VPN peer
  detail and SMART capabilities stayed stale for up to 5 s after a mutation;
  now `{ cache: false }`).

- Persistent notification center: the bell panel now keeps the full history
  with severity filters, an unread-only toggle and text search, and the
  "What's New" release-highlights modal shown once per version per account
  (`GET|POST /api/whats-new[/seen]`, migration `042_whats_new.sql`,
  `src/release_notes.rs`). See `docs/guides/notifications.md`.
- Storage health: a per-virtual-disk scrub-status endpoint + inline UI
  (`GET /api/storage/virtual-disks/{name}/scrub`) and foreign-pool import for
  ZFS/Btrfs pools discovered on attached disks (`GET /api/storage/importable-pools`,
  `POST /api/storage/import-pool`). See `docs/guides/storage.md`.
- Hardened app-proxy auth: proxied apps no longer receive the API session.
  The proxy gate accepts only an app-proxy-scoped `hv_app_session` token minted
  by the server-chosen `GET /app-auth` entry, and both HomeVault cookies are
  stripped before forwarding upstream. See `docs/guides/docker.md`.
- Versioned client/server contract: `GET /api/meta` + the
  `X-HomeVault-Contract` header, with a 426 + reload banner on a mismatch
  (`src/api_contract.rs`, `src/middleware/contract.rs`). See
  `docs/ARCHITECTURE.md`.
- Development tooling: `scripts/dev-instance.sh` (multi-instance `.local`
  dev via `HV_MDNS_HOSTNAME`), a real-VM Playwright suite
  (`playwright.vm.config.ts`, `tests/vm/*.vm.test.ts`), and
  `docs/guides/development.md`.

- External AI-agent server (MCP, Streamable HTTP): `POST /mcp`
  (`initialize`, `tools/list`, `tools/call`) with opaque `hv_mcp_…` bearer
  tokens, one master kill-switch and one global permission set shared by all
  tokens (apps / App Store / files / VMs / knowledge base / reboot — deny by
  default), managed from an admin-only "External agents" panel in the Chat app
  (`GET|PUT /api/mcp/status`, `PUT /api/mcp/permissions`, `POST|DELETE
  /api/mcp/tokens`). The `knowledge.search` tool exposes the internal RAG base
  to agents. See `docs/guides/mcp.md`.
- App catalog phase B (feature parity on the same `homevault-app.toml` shape):
  dependencies with `implements` alternatives + install-review endpoint,
  `[[environment]]` install settings rendered as `{{NAME}}`, `arch` +
  `min_homevault_version` compatibility (listed and enforced), per-app compose
  logs, and a sequential `update-all` queue. Install/uninstall share their core
  with the MCP `apps.*` tools. See `docs/guides/docker.md`.

- GitHub community health files: `LICENSE` (MIT), `SECURITY.md`,
  `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, issue/PR templates, Dependabot,
  `CODEOWNERS`, `.editorconfig`, `.gitattributes`.
- SPDX metadata (`license`, `repository`, `homepage`) in `Cargo.toml`,
  `frontend/package.json`, and root `package.json`.

### Added

- **Automatic snapshots of HomeVault's own database** (`src/db_backup.rs`,
  migration `048_app_meta.sql`). The database runs in WAL mode, so copying
  `homevault.db` on its own yields a file with **no tables at all** (measured:
  a 4 KB `.db` next to a 1.8 MB `-wal`) — the obvious manual backup silently
  produced nothing restorable. Snapshots are written with `VACUUM INTO`, which
  produces a single self-contained file: at startup when due, daily, before any
  migration is applied, and on demand. Settings → System → *Database snapshots*
  shows the last run, the schedule and the files on disk.
- **Liveness and readiness probes** — `GET /api/health/live` (public) and
  `GET /api/health/ready` (public, 503 when the database cannot be queried or
  was found damaged at boot). `GET /api/health` previously answered **HTTP 200**
  in both of those cases, so no monitor could detect an outage from it.

### Security

- **CSP no longer allows `'unsafe-inline'` scripts.** `script-src` lists a
  per-response nonce instead, and the single inline boot script in `index.html`
  carries it. With `'unsafe-inline'` plus the session token in `localStorage`,
  any injected script could read the token and exfiltrate a full session.
  `style-src` keeps `'unsafe-inline'` (Svelte scoped styles and `style=""`
  attributes cannot be nonce'd) — documented debt in `guides/security.md`.
- **The installer can roll back and uninstall.** `--rollback` swaps in
  `/opt/homevault/homevault.previous` (kept by every upgrade); `--uninstall`
  removes the app but **keeps** the data, and `--purge` deletes it behind a
  typed `PURGE` confirmation; `--dry-run` prints every action and changes
  nothing (and works without root). A failed release no longer leaves an
  appliance with no way back.
- **HTTPS now actually protects the login.** Enabling TLS left the plain-HTTP
  listener serving the wizard and the login form — admin password in clear — and
  `redirect` defaulted to **off**, so nothing changed in practice. It now
  defaults to **on** (opt out with `TLS_REDIRECT=0`), a legacy `data/tls.json`
  picks up the safe default via `serde(default)`, and the installer's HTTPS
  question defaults to **yes**. The session cookie gains `Secure` when the
  request arrived over TLS, and the wizard + login screen show a
  `role="status"` warning when served over plain HTTP on a non-loopback host.
- **Removed the dead `fail2ban` middleware.** `src/middleware/fail2ban.rs`
  defined an in-memory IP ban list that no route or handler ever referenced — it
  read like an active protection while doing nothing. Brute-force defence is the
  per-account lockout and the per-IP throttle (see below).
- **The login lockout is temporary again.** Five wrong passwords used to set
  `users.locked` and never clear it, so on a single-admin appliance a typo storm
  bricked the only account — the message said "temporarily locked" while the
  only ways back in were the offline recovery code or editing the database. A
  lock now records `locked_until` (migration `047`): an automatic lock lifts
  itself once the timestamp has passed and a successful login clears it too,
  while a deliberate admin lock (no `locked_until`) stays permanent. The
  migration repairs accounts already bricked by the counter.
- **Authentication is throttled per client IP.** The only counter was per
  account, so password spraying — one password tried across many usernames —
  was unlimited. `AuthService::check_ip_rate_limit` now runs before the account
  lookup on both `/auth/login` and `/auth/2fa/verify`: 30 attempts/minute and
  120/hour per IP, returning `429`. The handler honours that status instead of
  collapsing it into a 400.
- **The client IP is the TCP peer, not `X-Forwarded-For`.** Trusting the header
  outright let anyone forge the address recorded in the login history — and,
  now that the throttle is keyed by IP, sidestep it by rotating a header value.
  The headers are honoured only from a loopback peer (a reverse proxy on this
  host) or an address listed in `HOMEVAULT_TRUSTED_PROXIES`.
- **Login accessibility:** the inline error is now a `role="alert"` /
  `aria-live="assertive"` region, the credential fields carry `aria-invalid` +
  `aria-describedby`, and closing the error modal returns focus to the form
  instead of dropping it on `<body>`. The locked-account message points at the
  recovery-code path rather than "contact support".
- **The first-run bootstrap endpoints can no longer be driven from a foreign
  web page.** The CORS layer is permissive (`allow_origin(Any)`), so the
  local-network guard alone left a hole: the operator's browser *is* on the LAN
  and the mDNS name is predictable, so any site they visited could
  `POST /api/auth/setup` and claim the first admin. `middleware::setup_guard`
  now also refuses a foreign `Origin`, `Origin: null` and any `Sec-Fetch-Site`
  other than `same-origin`/`none` (403), and rate-limits the guarded routes to
  20 requests/min per peer (429). `HOMEVAULT_SETUP_ALLOW_REMOTE=1` no longer
  bypasses these two checks.
- **The setup lock is self-healing.** `.setup_lock` records `pid` +
  `created_at`; a dead PID or an age over 15 minutes makes it reclaimable
  (`mark_setup_in_progress` removes and re-creates it under `O_EXCL`), and
  `cleanup_stale_setup_lock()` runs at startup. A crash, an OOM kill or a power
  cut during setup used to dead-end the wizard permanently — `is_first_run`
  stayed true while every submission answered "Setup is already in progress".
- **Password policy raised to 12 characters, with a compromised-password
  check.** `SetupService::validate_password` is now the single policy (the
  wizard, admin-created users, password changes and recovery-code resets all
  delegate to it): 12–128 characters, mixed case + digit, not on the
  obvious/breached list (`Password1234`, `Homevault2026`, `qwerty…`, …) and no
  character repeated more than four times in a row. The wizard's live checklist
  lists all six rules and gates its "Next" button on them.
- **The recovery-code screen is blocking.** A failed generation now offers a
  retry instead of a "continue": the code is the only offline way back into the
  account (there is no email reset), so the wizard no longer lets an admin walk
  away without one.

### Removed

- **Setup Wizard: the storage, RAID, restore *and language* steps are gone** —
  the wizard is down to 4 steps (welcome, account, password, verify). RAID is
  configured from its own app, the appliance is installed by script on an
  existing OS, a backup is restored from the Backup app, and the interface
  language is now **detected from the OS/browser locale** (English when
  unsupported, changeable in Settings) instead of being asked during
  onboarding. Removed with them: `GET /auth/setup/storage-options`, the
  frontend `getSetupStorageOptions` service method, the
  storage/RAID/restore/language step markup in `SetupWizard.svelte`, the
  `setup.storage.*` / `setup.raid.*` / `setup.restore.*` / `setup.language.*`
  i18n keys in all four locales, and the matching integration test.
  `setup_progress.data` now carries nothing.

### Added

- **OS locale detection for the interface language**: `GET /auth/setup-status`
  reports `system_language` (`SetupService::detect_system_language()` reads
  `LC_ALL` → `LC_MESSAGES` → `LANG` → `LANGUAGE`, then `/etc/default/locale`),
  and `languageStore.applySystemLanguage()` uses it when the user made no
  explicit choice and the browser locale is not one HomeVault ships. Settings
  remains the only place a preference is stored.

### Fixed

- **An older HomeVault binary silently ran against a newer database.** Migrations
  are append-only and never reverted, so a downgrade started happily and then
  failed on the first query expecting a column it does not know about. Startup
  now compares `app_meta.schema_version` (migration `048`) with the highest
  migration embedded in the binary and **refuses to start** with a message that
  says what to reinstall — and `src/bin/server.rs` propagates the failure so the
  process exits non-zero (`systemd Restart=on-failure` can see it).
- **A damaged database was only discovered at the first query.** Startup runs
  `PRAGMA quick_check(1)` and surfaces a failure through `/api/health/ready`
  (`503 degraded`) instead of waiting for random queries to blow up.

- **The AI assistant could not answer about 25 of HomeVault's own features.**
  The RAG knowledge base indexed 20 feature documents while `docs/guides/` had
  grown to 46 files, so a question about Syncthing, the downloader,
  notifications, the desktop shell, LUKS encryption, RAID, the cache tier,
  widgets or a dozen other shipped features got "I don't know about that". The
  knowledge base now documents all 45 indexable guides — including the account
  page, login and recovery, the setup wizard, groups, antivirus,
  anti-ransomware, the firewall, LUKS encryption, the RAID wizard, snapshots,
  the cache tier, Syncthing, the downloader, the AI chat and its knowledge
  base, notifications, widgets, the desktop shell, the app launcher, themes,
  wallpapers, transfer quotas, mDNS, server control and avatars — and guide
  coverage is enforced by a test that enumerates `docs/guides/` instead of
  relying on the written rule.
- **Cloud Sync read and wrote a different rclone config than the one it parsed.**
  `GET`/`POST /rclone/config` and the add/delete-remote handlers hardcoded
  `/etc/rclone.conf`, while the remote list honoured the `RCLONE_CONFIG`
  override — so an admin whose config lives elsewhere saw a list and an editor
  describing two different files, and saving failed with `Permission denied`
  against the root-owned default. All four now go through a single
  `rclone_config_path()` resolver, which `parse_rclone_config()` uses too.
- **`JobType::Custom` could not be serialized, which silently broke progress
  reporting for every feature job.** `JobType` was internally tagged
  (`#[serde(tag = "type")]`) with a `Custom(String)` newtype variant, which
  serde cannot serialize at all. `handle_progress_sse` falls back to an empty
  payload on a serialization error, so a directory copy/move, Docker install,
  antivirus scan, RAG reindex, VM create, rescue, cloud sync or Ollama pull
  emitted an empty SSE `data:` line — the client's `JSON.parse` threw, no
  progress or completion callback fired, and the Explorer's directory
  copy/move reported failure for a job that had succeeded. `GET
  /api/progress/jobs` also panicked while any such job was tracked. Now uses
  serde **adjacent** tagging: built-in kinds keep their exact wire shape
  (`{"type":"backup"}`), feature jobs become
  `{"type":"custom","name":"explorer_copy"}`.
- **`GET /api/progress/{job_id}/status` returned a placeholder.** It answered
  a hardcoded `Running` / 0 % / `"Waiting..."` for any tracked id, so a job at
  87 %, a completed job and a failed job all read back as "running, 0 %". It
  now returns the job's stored last event — the same snapshot as the SSE
  stream and the activity feed — and 400s for an unknown or removed id.
- **Deleting a user now revokes the share permissions it held directly.**
  `handle_user_delete` previously only *logged* an intent to revoke: the
  `share_permissions` rows (keyed by grantee name, with no foreign key to
  `users`) and the real POSIX ACLs were both left in place, so recreating an
  account with the same username silently inherited the deleted user's access.
  The handler now calls `PermissionService::revoke_all_user_permissions`, which
  removes the filesystem ACLs (best-effort) and the database rows. Group grants
  are untouched — they belong to the remaining members. Regression guard:
  `tests/users_integration.rs` →
  `test_user_delete_revokes_its_direct_share_permissions_only`.

### Changed

- **One corner language across the shell, the apps and the gateway.** An
audit found the radius scale applied unevenly: the desktop widget/monitor
panels, the notification list rows and the floating path picker carried
literals off the appliance stops (28px power dialog, 20px notification
rows, 14/7/9px picker parts), a dead `--hv-radius-pill` token, an undefined
Tailwind `rounded-4xl` on the badge, and `calc(var(--radius) - 5px)` (1px)
inside the input group. Every corner now reads a stop from
`docs/guides/design-system.md` §3.2 — window `--hv-radius-2xl` (16px),
floating chrome panels `--hv-radius-4xl` (24px), cards/modals/pickers
`--hv-radius-xl`/`--hv-radius-card` (12px), controls/fields/buttons
`--hv-radius-control`/`--hv-radius-md` (8px), pills/dots
`--hv-radius-full`. The login and setup-wizard fields and buttons moved off
`rounded-lg` (10px, the card stop) onto the control stop so they match the
shared `Input`/`Button`. Guards: `desktop.spec.ts` 7.4 (power dialog ==
topbar popover = 24px), `login.spec.ts` 13.7 (setup card 16px / login field
8px). The `--hv-radius-4xl` documentation row was corrected (it never was
the login/setup card stop) and the stale `--hv-radius-sm` (2px) note in the
theme README was fixed.

- **The WebDAV server is now part of HomeVault (Rust), not a generated
  Python script + root systemd unit.** `src/webdav/` is an in-process axum
  listener built on `dav-server` (same hyper 1.x stack as the app proxy),
  managed by `WEBDAV` (`start`/`stop`/`reload`, mirrors `APP_PROXY`). It also
  fixes three real defects in the old implementation: Basic auth now actually
  verifies the HomeVault credentials (the Python `_check_auth` accepted any
  `Authorization: Basic` header), the URL's first segment is a **share name**
  confined to that share's root (the old server ignored `path_prefix` and
  served the process CWD as root), and `PROPFIND` is authenticated. The
  `share` grants of `resource_shares` are enforced per member on every
  operation, and `path_prefix` / `require_auth` / `require_ssl` / `locking` /
  `allowed_methods` are all honored (the default config is closed: port 8082 +
  auth on). An upgraded host disables and removes the legacy
  `homevault-webdav.service` + `homevault-webdav.py` at startup. See
  `docs/guides/shares.md`.

- **Static assets are now cached, validated and pre-compressed.** The
  `include_dir!`-embedded files used to be served as raw bytes with no
  `ETag` and no `Cache-Control` on `/wallpapers/*`, `/static/*` and the icon
  sprites, so a browser could never revalidate and re-downloaded several MB
  per visit. `spa.rs` now builds a `path → Asset` table once, sends an `ETag`
  (distinct per content coding), answers `If-None-Match` with `304`, and sets
  the cache lifetime per route — `immutable` for the content-hashed bundles,
  the wallpapers and `static/`; one day plus revalidation for the
  regenerated sprite; nothing for `index.html` / `sw.js`. The blanket
  `SetResponseHeaderLayer` for `/assets` and `/fonts` was replaced by this
  per-route policy. `frontend/scripts/precompress-dist.mjs` (npm `postbuild`)
  writes `.br` / `.gz` siblings that the server serves directly instead of
  re-compressing every response, and the `tower-http` `fs` + `set-header`
  features are no longer enabled.
- **The unified icon sprite is subset to what the frontend references**,
  dropping 7 750 glyphs to ~110 (3.6 MB → ~55 KB, and ~12 KB brotli), with
  `mapping-unified.ts` filtered to match (485 KB → 6 KB). Regeneration is now
  part of `npm run build` (`prebuild`); `HV_SPRITE_FULL=1 npm run
  icons:generate` restores the complete sprite.
- Release profile: `strip = true` and `panic = "abort"` (smaller binary; a
  panic now aborts the process instead of unwinding one task, so recovery
  relies on the systemd unit's `Restart=on-failure`).
- `installer/` is now a **single self-contained `install.sh`**. The systemd
  unit and the udiskie automount config are written inline; the sibling
  `homevault.service`, `setup-automount.sh`, `optional-packages.conf` and
  packaging `Makefile` are gone. The application binary is downloaded as a
  per-architecture asset (`homevault-<arch>.tar.gz`) from GitHub Releases
  (with optional SHA-256 verification), and `install.sh --package` builds
  those release assets from a checkout.
- **The login screen and the setup wizard now use the desktop's palette
  instead of a second identity of their own.** Both carried a page-scoped
  indigo/violet "gateway" palette — a second accent, a pastel gradient and a
  slate ink ramp one step lighter than the app. Every `--hv-login-*` /
  `--hv-wizard-*` value is now an alias of an Appliance token: the accent
  *is* `--hv-primary` (#1E62AE), the canvas *is* `--hv-desktop-bg` (declared
  once — `Desktop.svelte` no longer keeps its own copy of that gradient), the
  ink is `--hv-shell-text-*`, and the card borrows the desktop window's
  hairline and elevation. The wizard's emerald family (`#10B981` / `#22C55E` /
  `#16A34A` and its create gradient) failed WCAG 1.4.3 / 1.4.11 in every role
  it carried ink (2.28–3.77:1) and was replaced by the Appliance success ramp
  (`#1E7E34` / `#166428`); the amber recovery box is now white paper inside a
  `--hv-warning` border with an amber code chip, and the login screen uses one
  error red instead of two. `login.spec.ts` 13.6 asserts the aliases *and*
  every contrast floor of the gateway palette. See
  `docs/guides/design-system.md` §6.1/§6.2.

## [1.0.0] - 2026-10-10

### Added

- Rust (Axum) backend + Svelte 5 frontend NAS appliance: dashboard, users
  and groups, storage and RAID, shares (SMB/NFS/FTP/SFTP/WebDAV), file
  explorer with trash, restic backups, rclone cloud sync, network,
  antivirus and firewall, Docker with an app catalog, virtual machines
  (Incus), VPN (WireGuard/OpenVPN), Syncthing, aria2 downloader, local AI
  chat (Ollama) with a RAG knowledge base, widgets, health monitoring,
  system journal, transfer quotas, web terminal, and settings.
- SQLite migrations (`sqlx/`), installer (`installer/`), and reference
  documentation (`docs/`, `TESTING.md`, `ROADMAP.md`).

[Unreleased]: https://github.com/nicoleroux35/HomeVault/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/nicoleroux35/HomeVault/releases/tag/v1.0.0
