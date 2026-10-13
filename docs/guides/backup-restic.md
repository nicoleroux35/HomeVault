# Backup / Restic

**Purpose:** restic repositories, snapshots, manual backup/restore, and scheduled
backups with retention policies.

## Backend

Handlers in `src/handlers/restic.rs`. Scheduler in `src/restic_scheduler.rs`
(`compute_next_run`, `run_scheduler_loop`). Repos in
`src/storage/restic_repo.rs`, schedules in `src/storage/restic_schedule_repo.rs`,
retention in `src/storage/backup_retention_repo.rs`. Wrapper in `src/restic.rs`.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /restic/available`, `/restic/version` | auth | |
| `GET /restic/repos` | auth | `?include_password=` → admin only when true |
| `POST /restic/repos` | admin | create |
| `GET/PUT/DELETE /restic/repos/{id}` | auth/admin | |
| `GET /restic/repos/{id}/snapshots\|stats\|jobs` | auth | |
| `POST /restic/repos/{id}/unlock\|prune` | admin | |
| `POST /restic/backup` | admin | run backup (SSE progress) |
| `POST /restic/restore\|forget` | admin | |
| `POST /restic/check` | auth | integrity check (read-level session) |
| `POST /restic/snapshots/delete` | admin | |
| `GET /restic/schedules` | auth | list schedules |
| `POST /restic/schedules` | admin | create |
| `PUT/DELETE /restic/schedules/{id}` | admin | update/delete |
| `PUT /restic/schedules/{id}/enabled` | admin | enable/disable |
| `GET /restic/retention-policies` | auth | list |
| `POST /restic/retention-policies` | admin | create |
| `PUT/DELETE /restic/retention-policies/{id}` | admin | update/delete |
| `GET /restic/reminders` | auth | **backup reminders** — for each repo, `last_backup_at` + `days_since` + a `stale` flag, so the UI can nag when a repository has not been backed up recently |
| `GET /restic/restore-plan` | auth | **pre-flight space check** — `?repo_id=&snapshot=` → `{ size_bytes, available_bytes, fits }`, comparing the snapshot's total restore size (`restic::restore_size`) with the free space (`df`) on the destination. Debounced by the restore dialog before the user commits |

## Frontend

- `frontend/src/routes/Restic.svelte` (full i18n coverage; date locale follows UI
  language, not hardcoded `en-US`).
- `frontend/src/lib/services/api.ts`: all `restic*` methods including schedules
  and retention policies, plus `resticReminders` and `resticRestorePlan`.
- **Backup reminders banner** — the page opens with a warning when any repository
  is stale (`GET /restic/reminders`), naming the repository and how many days
  since its last successful backup.
- **Split-view restore dialog** — choosing a snapshot opens a two-pane restore
  view (snapshot tree on one side, destination on the other). Before the user
  commits, a **debounced pre-flight** calls `GET /restic/restore-plan` and shows
  the required vs. available space, warning when the snapshot will not fit.
- **Floating progress island** — backups/restores are tracked by a module-level
  Svelte store (`frontend/src/lib/stores/backupJobs.svelte.ts`) rendered by
  `frontend/src/lib/components/backup/BackupProgressIsland.svelte`, mounted once
  in `Desktop.svelte`. Because it lives above the app windows, a running backup
  keeps its progress visible while the user switches apps; `Restic.svelte`
  registers each run with `trackJob`.

## Invariants

- Every one of the 26 `restic.rs` handlers requires a session (a past bug accepted
  any non-empty token, and `?include_password=true` leaked every repo password
  in plaintext). Read = `require_auth`; mutations (incl. `include_password`) =
  `require_admin`.
- `handle_restic_backup_create` must call `update_backup_job` after
  `create_backup_job` on **both** success and failure paths, so jobs are never
  stuck on `running`.
- The scheduler computes `next_run_at` via the `cron` crate and reschedules (and
  marks `failed`) even when a run errors.
- Backup/restore/verify already emit `ProgressEmitter` progress, so they show in
  the desktop [activity taskbar](./activity.md) with no `restic.rs` change
  (`/api/activity` reads the same `PROGRESS_MANAGER` registry). The floating
  progress island reads the same store, so a run is visible both in the taskbar
  and over the desktop.
- **The restore pre-flight never blocks the restore.** `GET /restic/restore-plan`
  is advisory: if `restic` or `df` cannot be read, the dialog shows "unknown"
  rather than refusing — the real restore still runs and reports its own error.
- **Reminders are derived, not stored.** `days_since` is computed from the last
  successful backup timestamp (`restic_repo::last_successful_backup_at`); there
  is no separate reminder table to drift out of sync.
- **Editable since 2026-09-12** — `CLAUDE.md`'s frozen-area registry is gone, so
  `Restic.svelte`, the `restic*` api.ts methods, `src/handlers/restic.rs`,
  `src/restic_scheduler.rs`, the schedule/retention repos and the `restic.*`
  i18n section are open to edits. The invariants above stay binding: a change
  keeps the Rust integration + Playwright E2E suites green.

## Related

- [Snapshots](./snapshots.md), [Storage](./storage.md), [API Contract](../API.md)
