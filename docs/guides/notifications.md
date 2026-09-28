# Notifications

**Purpose:** Turn existing health checks and backup job results into
persisted, in-app notifications, with optional email and webhook dispatch.
Before this feature, `src/email.rs` was fully built but never called by
anything, and the frontend already had a working bell/badge/panel
(`notificationsStore`, `NotificationCenter.svelte`) that nothing ever
populated — this feature is the missing wire between the two.

## Backend

- `src/notification_engine.rs::run_notification_loop` — background loop
  spawned once in `web/mod.rs::run()` (same `tokio::spawn` + 60s
  `tokio::time::interval` pattern as `restic_scheduler::run_scheduler_loop`).
  Each tick:
  1. Calls the existing `HealthService::run_all_checks()` (`src/health.rs`,
     via `spawn_blocking` since it shells out synchronously) and compares each
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

## Frontend

- `frontend/src/lib/stores/notifications.ts` — `notificationsStore` gained
  `setFromServer(rows)`, which replaces the store from backend rows
  (preserving server-issued ids, so mark-read/delete round-trip correctly)
  instead of the old client-only `add()`.
- `frontend/src/lib/components/NotificationCenter.svelte` — mark-read/remove/
  mark-all/clear now also call the backend (`api.markNotificationRead`, etc.)
  instead of only mutating local state; hardcoded English strings replaced
  with `$t('notifications.*')`.
- `frontend/src/routes/Desktop.svelte` — on-mount + 30s-interval fetch of
  `api.getNotifications()` feeding `notificationsStore.setFromServer(...)`,
  cleaned up on destroy. The bell icon, unread badge, and panel were already
  wired up in the topbar (`showNotifications`, `$notificationsStore.unreadCount`)
  — this was the only missing piece.
- `frontend/src/routes/Settings.svelte` — Notifications card gained a webhook
  URL field + enable toggle; "Send test email" now reflects the real result.
- `src/i18n/{en,fr,de,es}.toml` — `[notifications]` section.

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
