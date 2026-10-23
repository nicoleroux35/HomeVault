# Notifications

**Purpose:** Turn existing health checks and backup job results into
persisted, in-app notifications, with optional email and webhook dispatch.
Before this feature, `src/email.rs` was fully built but never called by
anything, and the frontend already had a working bell/badge/panel
(`notificationsStore`, `NotificationCenter.svelte`) that nothing ever
populated — this feature is the missing wire between the two.

**Per-event control (2026-10-13):** every notification the engine raises
carries a stable **event id** (`backup.jobs`, `snapshot.schedules`,
`app.updates`, `health.<check>` — see `src/notification_events.rs::EVENT_CATALOG`); the
admin can mute events individually (`GET/PUT /api/notifications/events/config`,
admin-only; Settings → Notifications). A muted event is neither persisted
nor dispatched. Absent toggle = enabled, so the catalog can grow without
silencing existing installs. The webhook also gained a **sink format**
(`generic` | `slack` | `discord`) so a Slack/Discord incoming-webhook URL
can be pasted without a relay — the payload builder matches each service's
expected shape.

## Backend

- `src/notification_engine.rs::run_notification_loop` — background loop
  spawned once in `web/mod.rs::run()` (same `tokio::spawn` + 60s
  `tokio::time::interval` pattern as `restic_scheduler::run_scheduler_loop`).
  Each tick:
  1. Calls the existing `HealthService::run_all_checks(&pool)` (`src/health.rs`,
     async — the service self-offloads the system checks to `spawn_blocking`
     internally, so the engine must NOT re-wrap the call) and compares each
     check's status against an in-memory set of "currently open problems" to
     detect **transitions** (a check turning `Warning`/`Unhealthy`, or
     recovering) — so the same ongoing problem doesn't re-notify every minute.
  2. Read-only polls `ResticRepository::list_backup_jobs(repo_id, 5)` for
     every repo and compares against an in-memory "last-seen job id" map,
     notifying only on a genuinely new `completed`/`failed` job.
     **`src/restic_scheduler.rs` itself is never modified or called** — it's
     owned by the Backup page — this is a pure read from the same
     `restic_backup_jobs` table it already writes to.
  3. On any new notification, persists a row via
     `NotificationRepository::create`, then — if
     `NOTIFICATION_SETTINGS.enable_alerts` and the severity meets
     `alert_threshold` — sends an email via the now-wired-up
     `EmailService::send_system_alert` and/or posts a JSON payload to
     `NOTIFICATION_SETTINGS.webhook_url` if `webhook_enabled`.
- `src/storage/notification_repo.rs` — `notifications` table (migration
  `015_notifications.sql`). Broadcast-style (no `user_id` — matches
  `NotificationSettings`/`EmailSettings` already being global, not per-user).
- **Event sources other than the loop.** `notify()` is `pub(crate)` so a
  background engine can raise an event through the same mute/persist/dispatch
  path instead of writing notification rows directly. Current caller:
  `snapshot_scheduler::record_failure` raises `snapshot.schedules` (severity
  `error`, category `storage`) **once per outage** — on the transition into
  failure, so a schedule that keeps failing does not notify every tick (see
  [Snapshots](./snapshots.md)).
- `src/handlers/notifications.rs` — list/unread-count/mark-read/mark-all-read/
  delete (see [API Contract](../API.md)), all `require_auth`.
- `src/email.rs` — `EmailService::send_system_alert` / `send_test_email` now
  take an explicit `&EmailConfig` parameter (built from `EMAIL_SETTINGS`)
  instead of silently using `EmailConfig::default()` — previously they always
  ignored whatever SMTP settings the admin had configured.
- `src/handlers/settings.rs::handle_email_test` — previously a stub that only
  logged and always reported success; now actually calls
  `EmailService::send_test_email` against the configured SMTP settings and the
  `NOTIFICATION_SETTINGS.email` recipient, and surfaces real failures.
- `NotificationSettings` (`src/handlers/settings.rs`) gained `webhook_url` /
  `webhook_enabled`, threaded through `src/settings_persistence.rs`
  (`PersistedNotifications`) so they survive a restart like every other
  setting.

### Email transport — the saved SMTP settings are the transport (ROADMAP 13.3)

Before this the SMTP fields were persisted and never used: `EmailService` only
passed `--from` to `msmtp`, so the admin's server/port/credentials were ignored
unless they happened to be in msmtp's own config file.

- `EmailConfig::from(&EmailSettings)` is the single mapping from the Settings
  card to the transport, used by both the test button and the notification
  engine. `EmailConfig::is_explicit()` says whether an SMTP relay was
  configured at all.
- With a relay configured, every send writes a private (`0600`) msmtp
  configuration to `data_dir()/msmtp.conf` (`msmtp_config_path` /
  `msmtp_config_text` / `write_msmtp_config`) and runs
  `msmtp --file <path> --from <addr>`. Host, port, username, password and TLS
  all come from the saved settings; a saved `smtp_server` therefore takes
  effect on the next notification, with no package or manual `~/.msmtprc` edit.
- A blank or `localhost` host keeps the legacy local-MTA path
  (`mutt` → `sendmail` → `msmtp`) for appliances that already deliver mail
  through a host MTA. The card's hint says so.
- The generated file is `0600` and never returned by an API; the SMTP password
  stays redacted in the export and absent from `GET /settings/email`.
- stderr is captured by a reader thread while the child runs, so the 30 s
  deadline cannot be defeated by an MTA that writes steadily, and the last
  lines of stderr are what the error message shows.
- `is_email_configured()` probes the three binaries once and caches the answer
  instead of shelling `which` three times per call.
- Sending runs in `tokio::task::spawn_blocking` from the engine (ROADMAP X.1) —
  a hung MTA no longer blocks the scheduler tick.

### Webhook delivery (ROADMAP 13.5)

- `handlers::settings::validate_webhook_url()` runs at save time **and** again
  at dispatch, so a value persisted before this validation (or hand-edited into
  `settings.json`) can never reach `reqwest`. Rules: `http`/`https` only, a host
  is required, no embedded `user:pass@`, no whitespace/control characters, at
  most 2048 characters. Empty clears the setting.
- One `reqwest::Client` (`webhook_client()`, `OnceLock`) is shared by every
  alert: 10 s total timeout, 5 s connect timeout, HomeVault user agent. The old
  code built `Client::new()` per dispatch with no timeout at all, so a sink
  that accepted the connection and never answered pinned the scheduler task.
- A non-2xx answer is logged with its status; a delivery failure never fails the
  notification itself (the in-app row is already written).

## Frontend

- `frontend/src/lib/stores/notifications.ts` — `notificationsStore` gained
  `setFromServer(rows)`, which replaces the store from backend rows
  (preserving server-issued ids, so mark-read/delete round-trip correctly)
  instead of the old client-only `add()`.
- `frontend/src/lib/components/NotificationCenter.svelte` — the persistent
  notification **center**: mark-read/remove/mark-all/clear call the backend
  (`api.markNotificationRead`, etc.) instead of only mutating local state, and
  the panel carries severity-filter chips, an unread-only toggle and a text
  search. All strings come from `$t('notifications.*')`.
- `frontend/src/routes/Desktop.svelte` — on-mount + 30s-interval fetch of
  `api.getNotifications()` feeding `notificationsStore.setFromServer(...)`,
  cleaned up on destroy. The bell icon, unread badge, and panel were already
  wired up in the topbar (`showNotifications`, `$notificationsStore.unreadCount`)
  — this was the only missing piece.
- `frontend/src/routes/Settings.svelte` — Notifications card gained a webhook
  URL field + enable toggle; "Send test email" now reflects the real result.
- `src/i18n/{en,fr,de,es}.toml` — `[notifications]` section.

## What's New (release highlights)

A second, distinct surface: after an update the SPA shows the running
version's highlights once per account.

- `src/release_notes.rs` — a curated, static table of `ReleaseNote`s (one per
  released version, newest first). `current()` returns the entry whose
  `version` equals `CARGO_PKG_VERSION`; a missing entry simply means "no
  modal". **Release checklist:** add one entry for the version in `Cargo.toml`
  before tagging. Curated rather than parsed from `CHANGELOG.md` so the modal
  shows short, user-visible wins and can never break on a changelog heading
  change.
- `GET /api/whats-new` — `{ version, title, highlights, seen }`; `seen` is
  **per user** (`whats_new_seen` table, migration `042_whats_new.sql`), so the
  modal stays dismissed across browsers and devices.
- `POST /api/whats-new/seen` — records the acknowledgement (idempotent).
- `frontend/src/lib/components/WhatsNewModal.svelte` — uses the shared
  `theme/Modal` (bits-ui Dialog) for focus trapping, Escape and focus
  restoration. `Desktop.svelte` fetches once on mount and opens it only when
  the server says `seen: false` and there is at least one highlight.

## Versioned client/server contract

The SPA and backend are released together, but a browser can hold a cached
`app.js` from the previous build after an update. A versioned contract makes
that fail loudly instead of silently misparsing.

- `src/api_contract.rs` — `CONTRACT_VERSION` (bumped **only** on a breaking
  API change) + `client_is_compatible` (missing/non-numeric header = legacy,
  allowed; present-and-different = incompatible).
- `GET /api/meta` (unauthenticated) — `{ contract_version, server_version }`,
  the discovery endpoint the SPA uses to detect a mismatch before it has a
  session.
- `src/middleware/contract.rs` — rejects an explicit mismatch on `/api/*`
  with **426 Upgrade Required** (`/api/meta` exempt).
- `frontend/src/lib/services/api.ts` — sends `X-HomeVault-Contract` on every
  request and flags a 426 into `stores/contract.ts`; `Desktop.svelte` shows a
  reload banner (`[data-testid="contract-mismatch-banner"]`).

## Invariants

- Notifications are deduplicated by in-memory state that resets on server
  restart — a still-open problem may re-fire once after a restart. This is an
  accepted MVP limitation (no extra table for cross-restart dedup state).
- The engine's backup-job polling is strictly read-only against tables
  `restic_scheduler.rs` also writes — never edit `restic_scheduler.rs` to
  "improve" this integration; extend `notification_engine.rs` instead.
- Email/webhook dispatch happens only if the notification's severity meets
  `NOTIFICATION_SETTINGS.alert_threshold` (0–100, bucketed into
  info/warning/error/critical ranges) **and** `enable_alerts` is on; SMTP also
  requires `smtp_enabled`, webhook requires `webhook_enabled`.

## Related

- [Settings](./settings.md), [Health](./health.md),
  [Backup / Restic](./backup-restic.md), [API Contract](../API.md)
