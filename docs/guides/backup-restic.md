# Backup / Restic

**Purpose:** restic repositories, snapshots, manual backup/restore, scheduled
backups with retention policies — and snapshots of **HomeVault's own database**.

## HomeVault's own database (2026-10-17)

HomeVault's SQLite database (`{DATA_DIRECTORY}/data/homevault.db`) holds
accounts, settings, quotas, schedules and sessions. It runs in **WAL mode**, so
copying `homevault.db` on its own is **not a backup**: every committed
transaction may still sit in the `-wal` sidecar, and a copied `.db` file alone
can open with *no tables at all* (measured: a 4 KB `.db` next to a 1.8 MB
`-wal`).

Snapshots solve that. They are written with `VACUUM INTO`, which produces a
single **self-contained** file — restoring one needs neither `-wal` nor `-shm`.

| What | Where |
|---|---|
| Snapshots | `{DATA_DIRECTORY}/data/backups/homevault-YYYYMMDD-HHMMSS.db` |
| Service | `src/db_backup.rs` (`DbBackupService`) |
| Handlers | `src/handlers/db_backup.rs` |
| Settings | `data/db_backup.json` (`interval_hours`, `retain`) |

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /settings/backup/db` | admin | settings + every snapshot on disk + total size + `last_error` |
| `PUT /settings/backup/db` | admin | update the interval / retention (applies the new retention immediately) |
| `POST /settings/backup/db/now` | admin | take a snapshot now |

Behaviour:

- **At startup** — when the database existed and the newest snapshot is older
  than the interval, one is taken (best-effort; a failure never blocks boot).
- **Daily** — the scheduler loop snapshots when due and also folds the WAL back
  with `PRAGMA wal_checkpoint(TRUNCATE)`, so `-wal` cannot grow without bound.
- **Before any migration** — when pending migrations exist, a
  `homevault-…pre-migration.db` snapshot is written first, so an upgrade always
  has a point to come back to.
- **Retention** — only the newest `retain` *regular* snapshots are kept (default
  7, 1–100); pruning runs after every snapshot and immediately after a settings
  change. Settings persist in `data/db_backup.json` and survive restarts.
- **Serialised process-wide** — see below.
- **A failed attempt leaves its reason behind** — see below.

### Only one snapshot runs at a time (2026-10-24)

Every HTTP request builds its own `DbBackupService` (the schedule lives in
`data/db_backup.json`), so a per-instance lock serialises nothing: a manual click
landing while the scheduler was snapshotting used to produce **two concurrent
`VACUUM INTO`s**, which is exactly what this guide promised could not happen
(ROADMAP 22.1). The lock is now a module static in `src/db_backup.rs`
(`RUNNING`), shared by every handler, the scheduler loop, the startup snapshot
and the pre-migration primitive. A unit test holds the lock and proves a
snapshot waits for it.

The write still goes to a `.tmp` name and is renamed into place, so an
interrupted snapshot never looks restorable. Two snapshots can legitimately
share a second (a click right after a scheduled run); the second one is named
`homevault-…_2.db` rather than overwriting the first, and the `_` suffix keeps
the reverse-name sort reading as reverse-chronological.

### Retention never eats the pre-upgrade restore point (2026-10-24)

`retain` bounds the **regular** snapshots only. The old `prune` kept the newest
`retain` *files by name*, without a marker filter, so a
forced `homevault-<ts>.pre-migration.db` — the only copy of the pre-upgrade
state — could be deleted like any other snapshot, and it also consumed one of
the retention slots (ROADMAP 22.2).

Pruning now partitions the directory:

| Kind | Bounded by |
|---|---|
| regular snapshots (scheduled, manual) | `retain` (1–100, default 7) |
| `.pre-migration.db` restore points | `PRE_MIGRATION_RETAIN` = 3, newest first |

A restore point is never removed by the `retain` pass, so an upgrade always has a
point to come back to even when the operator keeps two snapshots. They are still
bounded: an appliance that upgrades every week cannot fill the data volume with
them. `DbBackupEntry.is_pre_migration` marks them in the API, and the Settings
card labels them *Before upgrade*.

### A failure is remembered

`DbBackupStatus.last_error` used to be hardcoded `None`, so an unwritable
directory, a full disk or a locked file looked identical to "no snapshot yet" —
the card could never show a failed attempt (ROADMAP 22.2). The reason of the
most recent failed attempt is now kept in a process-wide slot, served by
`GET /settings/backup/db`, cleared by the next success and by a restart, and
displayed on the card as a `role="alert"` line quoting the server's own message.
It is in-process on purpose: the status is built by a throw-away service per
request, and the card that offers the button is where the failure has to be
visible.

### Nothing blocks the async runtime (2026-10-24)

Every `std::fs` call of the service (`create_dir_all`, the stale `.tmp` removal,
the final `rename`, `metadata`, `read_dir`, `remove_file` in the retention pass,
and the `data/db_backup.json` read/save/delete) runs on `spawn_blocking`
(ROADMAP X.1), and `status()` / `list()` / `prune()` / `is_due()` are `async`
because they reach the filesystem. `DbBackupService::load(db).await` is the
constructor handlers use — `new()` keeps the shipped defaults in memory and does
no I/O, which is what the unit tests use.

### Restoring

1. `systemctl stop homevault`.
2. Copy the snapshot over the live database:
   `cp {DATA_DIRECTORY}/data/backups/homevault-YYYYMMDD-HHMMSS.db {DATA_DIRECTORY}/data/homevault.db`
   and delete any leftover `homevault.db-wal` / `homevault.db-shm`.
3. `systemctl start homevault`.

Point the `DATA_DIRECTORY` at a persistent volume and the snapshots — like the
database itself — survive a reinstall of the system disk.

## Backend

Handlers in `src/handlers/restic.rs`. Scheduler in `src/restic_scheduler.rs`
(`compute_next_run`, `run_scheduler_loop`). Repos in
`src/storage/restic_repo.rs`, schedules in `src/storage/restic_schedule_repo.rs`,
retention in `src/storage/backup_retention_repo.rs`. Wrapper in `src/restic.rs`.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /restic/available`, `/restic/version` | auth | |
| `GET /restic/repos` | auth | `?include_password=` → admin only when true |
| `POST /restic/repos` | admin | create + `restic init` |
| `GET/PUT/DELETE /restic/repos/{id}` | auth/admin | |
| `GET /restic/repos/{id}/snapshots\|stats\|jobs` | auth | |
| `POST /restic/repos/{id}/unlock\|prune` | admin | |
| `POST /restic/backup` | admin | **starts** a backup, answers `{status:"running", job_id, backup_job_id}` |
| `POST /restic/restore` | admin | **starts** a restore, answers `{status:"running", job_id}` |
| `POST /restic/forget` | admin | at least one retention rule required |
| `POST /restic/check` | auth | metadata check for any session; `read_data: true` (re-reads every pack) is **admin** |
| `POST /restic/snapshots/delete` | admin | |
| `GET /restic/schedules` | auth | list schedules |
| `POST /restic/schedules` | admin | create |
| `PUT/DELETE /restic/schedules/{id}` | admin | update/delete |
| `PUT /restic/schedules/{id}/enabled` | admin | enable/disable |
| `GET /restic/retention-policies` | auth | list |
| `POST /restic/retention-policies` | admin | create (at least one limit, values > 0) |
| `PUT/DELETE /restic/retention-policies/{id}` | admin | update/delete |
| `GET /restic/reminders` | auth | **backup reminders** — for each repo, `last_backup_at` + `days_since` + a `stale` flag, so the UI can nag when a repository has not been backed up recently |
| `GET /restic/restore-plan` | admin | **pre-flight space check** — `?repo_id=&snapshot_id=&target=` → `{ required_bytes, available_bytes, fits }`, comparing the snapshot's total restore size (`restic::restore_size`) with the free space (`df`) on the destination. Debounced by the restore dialog before the user commits |

### Long-running runs are background jobs (2026-10-21)

`POST /restic/backup` and `POST /restic/restore` used to run restic **inline**:
the HTTP request stayed open for the whole run (hours on a multi-TB dataset),
`trackJob` was only called once the job had already finished (so the floating
island showed a completed job), a browser timeout looked like a failed backup,
and the Tokio worker was parked for the duration.

Both now return immediately:

```json
{ "status": "running", "job_id": "…", "backup_job_id": "…", "repository": "nas" }
```

- the run happens in a spawned task, with the blocking restic call on
  `spawn_blocking` (`ResticService::run_restic_op`);
- `restic backup --json` is read **line by line**
  (`ResticService::create_backup_streaming`), so `percent_done` / `bytes_done`
  become real SSE progress (`ResticProgress` → `ProgressEmitter::progress_extended`)
  instead of the old 5 % → 90 % jump;
- `DELETE /api/progress/{job_id}` really stops the process: the child is
  registered in `ResticService`'s run registry and the cancel hook sends
  **SIGINT** (restic then releases its repository lock), with SIGKILL after 10 s
  as a watchdog. The job ends `cancelled`, not `failed`.
- **One run per repository.** `RepoRunLock` refuses a second backup/restore on
  the same repo with "A backup or restore is already running on this
  repository" instead of letting restic answer "repository is already locked"
  after a full source scan. Two different repositories may run in parallel.
- the `restic_backup_jobs` row is written **before** the run (status
  `running`), so `started_at` is the real start time and the run is visible
  while it happens. `snapshot_id` is filled in when restic reports it.
- the scheduler closes every leftover `running` job at startup
  (`fail_stale_running_jobs`): a crash or a `systemctl stop` mid-backup used to
  leave a phantom run in the history for ever.

`POST /restic/check` stays synchronous (the dialog shows the verdict inline),
but it also runs off the runtime and is cancellable. `--read-data` re-reads
every pack file — hours of disk I/O — so it requires admin.

### Arguments are validated before restic is spawned

Every value that would become an argv entry is checked in `src/restic.rs`:
`validate_snapshot_id`, `validate_backup_path`, `validate_restore_target`,
`validate_snapshot_name`, `validate_repository`, `validate_password`,
`validate_display_name`, `validate_keep_within`, `validate_tags`,
`validate_exclude_patterns`.

- `restic forget <snapshot_id>` with `snapshot_id = "--keep-last=0"` was a
  **full repository wipe**: the value was passed positionally and parsed as an
  option. Snapshot ids are now limited to `[A-Za-z0-9._-]`, no leading `-`.
- backup sources and restore targets must be **absolute** (which also makes a
  leading `-` impossible), free of control characters and of `..`.
- the pre-backup filesystem snapshot name builds `/tmp/restic-snap-<name>`, so
  a name of `../../etc` used to mount over an arbitrary directory.
- `forget` with no rule at all is refused here instead of coming back as
  restic's "Fatal: no policy was specified".
- repository create/update refuse an empty name, an empty location, an empty
  password, a relative source path and an unknown `retention_policy_id`.

### The check verdict

`restic check` prints **"no errors were found"** on success — a line that
contains the substring `error`. The old collector kept every line containing
`error`, so `healthy` was `false` on *every* healthy repository. The verdict now
comes from `ResticService::parse_check_output`: the success sentinel is
excluded, only `error:` / `fatal:` / `unable to` / `failed to` / `is damaged`
lines count, and a non-zero exit with no recognizable marker still reports one.

### Retention: count, age and size

The scheduler applies the repository's policy (or the default `apply_to_all`
policy) after each successful run:

1. `max_count` → `--keep-last`, `max_age_days` → `--keep-within <n>d`;
2. `max_size_gb` → measured with `restic stats`, then a `--keep-last` derived
   from the average snapshot size (`plan_keep_last_for_size`), followed by
   `forget --prune`.

`max_size_gb` used to be stored, returned by the API and **never applied**. The
size pass never keeps 0 snapshots and never deletes the newest one: a single
snapshot over the cap is left alone (with a log line) rather than wiped.

### Pre-backup filesystem snapshots

`snapshot_create_before_backup` mounts a ZFS/Btrfs snapshot at
`/tmp/restic-snap-<name>` and backs that up. A failed mount now **aborts the
run**: it used to be ignored, so the backup silently captured an empty mount
point, reported success and deleted the snapshot — the user believed their data
was safe. `SnapshotMount` (RAII) unmounts and removes the directory on every
exit path.

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
  registers each run with `trackJob(jobId, label, onSettle)` and `onSettle`
  reloads the snapshot/job lists when the run really ends (the start endpoint
  answers before the run, so reloading at that moment would show the previous
  state).

## Invariants

- Every one of the 26 `restic.rs` handlers requires a session (a past bug accepted
  any non-empty token, and `?include_password=true` leaked every repo password
  in plaintext). Read = `require_auth`; mutations (incl. `include_password`) =
  `require_admin`; `check` is `require_auth` but `read_data: true` is admin.
- **No blocking restic call on a Tokio worker.** Every handler goes through
  `ResticService::run_restic_op` (`spawn_blocking`, optional timeout:
  `RESTIC_QUERY_TIMEOUT` for metadata, `RESTIC_INIT_TIMEOUT` for `init`, none for
  the data-moving operations, which rely on cancellation instead).
- **The backup job row exists before the run.** `create_backup_job` (status
  `running`) → run → `update_backup_job` (`completed` / `failed` / `cancelled`,
  plus the `snapshot_id` restic reported). The startup reaper
  (`fail_stale_running_jobs`) closes anything left `running`.
- **One backup/restore per repository** (`RepoRunLock`), shared by the manual
  handlers and the scheduler. It is released by `Drop`, so an early return or a
  panic cannot wedge a repository.
- **Nothing unvalidated reaches argv.** New operations must validate every
  positional argument and every path with the `validate_*` helpers in
  `src/restic.rs`; `snapshot_id` in particular is never passed through as-is.
- The scheduler computes `next_run_at` via the `cron` crate and reschedules (and
  marks `failed`) even when a run errors — a single failure must not wedge a
  schedule with a stale `next_run_at`. It also skips (and reschedules) a run
  when another run already owns the repository.
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
