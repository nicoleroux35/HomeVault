# Cloud Sync (rclone)

**Purpose:** Manage rclone remotes, mounts, and sync/copy jobs to cloud storage.

## Backend

Handlers in `src/handlers/rclone.rs`. Every handler takes `headers: HeaderMap`
and checks a real session before doing anything — local `check_session`/
`check_admin` helpers (`Result<String, String>`, since these handlers return
`Json<ApiResponse<T>>` rather than `axum::response::Response`, so the shared
`admin_guard::require_auth`/`require_admin` couldn't be reused directly
without a large mechanical rewrite of every return point).

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /rclone/status` | auth | `{ available, fusermount, mount_base }` |
| `GET /rclone/remotes` | auth | configured remotes |
| `GET /rclone/providers` | auth | supported providers |
| `GET /rclone/mounts` | auth | active mounts (+ pid when this process owns them) |
| `POST /rclone/remotes` | admin | add remote |
| `DELETE /rclone/remotes/{name}` | admin | remove remote |
| `POST /rclone/mount/start` | admin | mount — body `{ remote, mount_point, path?, read_only?, allow_other? }` |
| `POST /rclone/mount/stop` | admin | unmount — body `{ mount_point }` **only** |
| `POST /rclone/sync\|copy` | admin | transfer jobs |
| `GET /rclone/config` | admin | raw config — exposes every remote's credentials, same sensitivity bar as Restic's `?include_password=true` |
| `POST /rclone/config` | admin | save raw config (0600, dated backup) |
| `GET /rclone/jobs` | auth | list persisted sync jobs |
| `POST /rclone/jobs` | admin | create a persisted sync job |
| `DELETE /rclone/jobs/{id}` | admin | delete a sync job |
| `POST /rclone/jobs/{id}/run` | admin | "Run now" — awaits the run in the request, then returns the **updated job** (schedule + recorded outcome) |
| `PUT /rclone/jobs/{id}/schedule` | admin | set or clear the cadence: `{ schedule_kind: none\|interval\|cron, interval_minutes?, cron_expr? }` |

`POST /rclone/crypt` was removed entirely — it was a fake stub (always
returned a canned "Crypt operation configured" string without doing
anything) with zero frontend callers, confirmed via a repo-wide grep before
deleting it.

Sync jobs are persisted server-side (`sqlx/migrations/036_sync_jobs.sql`,
table `sync_jobs`); the frontend used to keep them in `localStorage` (rows
vanished on another browser/profile) and now reads/writes them through the API
above. Read-only listing requires a session; create/delete/run are admin
mutations (`check_session`/`check_admin` in `src/storage/sync_job_repo.rs` +
`src/handlers/rclone.rs`).

## Deadlines — no rclone call may hang a request (ROADMAP 16.1)

Every subprocess goes through `crate::utils::process::run_timed_async_result`
(`src/utils/process.rs`), which kills the child at the deadline and returns a
**named** failure — `Spawn` ("not installed") vs `Timeout` ("too slow") — so
the two read differently in front of a user. The deadlines live in
`src/handlers/rclone.rs`:

| Constant | Value | Covers |
|----------|-------|--------|
| `RCLONE_PROBE_TIMEOUT` | 10 s | `rclone version`, `rclone listremotes`, `fusermount -V` (`/status`, `/remotes`) |
| `RCLONE_MOUNT_READY_TIMEOUT` | 20 s | how long a started mount may take to appear in the mount table |
| `RCLONE_UMOUNT_TIMEOUT` | 30 s | each of `fusermount -u` and `umount -f` |
| `RCLONE_TRANSFER_TIMEOUT` | 6 h | one `rclone sync` / `rclone copy` (and "Run now") |

`RCLONE_TRANSFER_TIMEOUT` is deliberately generous: a first sync of a large
library legitimately takes hours, and a shorter ceiling would turn a working
transfer into a failure. It is a **ceiling**, not a progress estimate — a
transfer that exceeds it is killed and the job is marked failed with the
reason. Before this, a stalled remote held the HTTP request *and* the
`cloud_sync` activity job open forever.

## Mount lifecycle — the process owns what it starts (ROADMAP 16.2/16.3)

`handle_mount_start` no longer appends `--daemon`, calls `spawn()` and forgets
the handle. It:

1. validates the request (`validate_mount_point`, `validate_remote_name`,
   `validate_remote_subpath`) **before** anything is created or spawned — the
   mount point must be an absolute path under the base with no `.`/`..` segment
   and no control character (a trailing newline used to be removed by `trim()`
   before the check ran, and `Path::components()` hides a `.` segment);
2. requires the remote to exist in the resolved config (`load_remotes`);
3. refuses a mount point this process already owns or that is already in
   `/proc/self/mounts` (one FUSE server per directory);
4. creates the directory off the async runtime (`spawn_blocking`);
5. spawns `rclone mount … --vfs-cache-mode writes --log-level ERROR` with
   `kill_on_drop(false)`, keeps the `Child` in a **process-wide registry**
   (`static MOUNTS`, keyed by mount point — the same shape as the downloader's
   `Aria2Process` in `AppState`, but a static so the 40+ `AppState` literals in
   `tests/` did not have to grow a field), and drains stderr into a bounded
   tail for the failure message;
6. returns success only once `wait_until_mounted` sees the mount point in
   `/proc/self/mounts` — a mount that "started" but never appeared is an
   error quoting rclone's last stderr lines, and the child is killed.

`handle_mount_stop` takes `{ mount_point }` (`MountStopRequest`), tries
`fusermount -u` then `umount -f` (both bounded, both reasons reported on
failure), then kills and reaps the child through `release_mount`.

`shutdown_mounts()` runs from `shutdown_signal` in `src/web/mod.rs`: every
tracked mount is unmounted, then its child killed. A restart therefore no
longer leaves stale FUSE mounts and orphaned `rclone` processes — the state
`--daemon` guaranteed.

`handle_list_mounts` reads `/proc/mounts` (whole-field match, `\040`-style
escapes decoded) and enriches each entry with the remote and pid from the
registry; a tracked child whose mount disappeared is reported
`is_active: false` so the screen offers a real "Stop".

## The config file holds credentials (ROADMAP 16.3)

- **Mode 0600, always.** `write_private_config` opens with `.mode(0o600)` and
  re-applies the mode afterwards, which also repairs a file an older build left
  at 0644. The dated `.backup.*` file is written the same way (the old
  `tokio::fs::copy` recreated it with the source mode). `CONFIG_FILE_MODE` is
  the single constant.
- **Parsed section edits, not string surgery.** `config_has_section` matches a
  whole trimmed header line, and `remove_section` ends a section at the next
  line that *starts* with `[`. Previously `contains("[my]")` was true for
  `[mydrive]` (adding `my` refused as a duplicate, deleting `my` could delete
  the wrong bytes) and the delete path ended the section at the first `[`
  anywhere, so a value like `patterns = [a-z]` truncated it.
- **The provider type is validated too.** `validate_provider_slug` (lower-case
  slug, ≤32 chars) because `type = {provider}` is written verbatim: a newline
  in that field appended arbitrary INI lines, the same injection class as the
  `[section]` guard on the free-text `config` field.
- **The remote name is validated** (`validate_remote_name`): no `:`, `[`, `]`,
  `/`, `\`, whitespace, control characters or leading `-` (an argv parser could
  read it as a flag), ≤64 chars.
- **Mount points are confined to `RCLONE_MOUNT_BASE`** (`/mnt/rclone`), as a
  strict child, with no `.`/`..` component — enforced server-side, because the
  API is reachable without the screen. `GET /rclone/status` returns
  `mount_base` so the screen derives its mount point from the same rule instead
  of hardcoding `/mnt/rclone/<name>`.
- **No blocking `std::fs` on the async runtime.** The config read happens in
  `spawn_blocking` (`load_remotes`), as does `create_dir_all`.
- **Subprocesses always receive `--config` explicitly.** All subprocess
  invocations (`rclone mount`, `rclone sync`, `rclone copy`, and fallback
  `rclone listremotes`) pass `--config <rclone_config_path()>` via
  `rclone_command()`. This guarantees that the spawned CLI processes access the
  exact same configuration file that the backend reads and writes (honouring
  `RCLONE_CONFIG` overrides or the `/etc/rclone.conf` system default), rather
  than falling back to `$HOME/.config/rclone/rclone.conf`, which may not exist
  or may belong to another user.

## Frontend

- `frontend/src/routes/CloudSync.svelte` — fully wired through `$t()`/
  `translate()` (previously had zero i18n calls anywhere in the file,
  ignoring the selected language entirely, the same bug class already found
  on Dashboard/Network/Shares/Chat/Explorer).
- Local source/destination path picking uses the shared
  `frontend/src/lib/components/theme/FolderPicker.svelte` (already
  integrated into Explorer) — a real filesystem browser instead of the
  previous "Local" mode misapplying the cloud-remote-name `<select>` as a
  filesystem path.
- **One page-level failure alert** (`data-testid="cloudsync-error"`,
  `rclone.error_title` + the server's reason, dismissible with
  `common.dismiss_banner`). Seven actions ended in an empty `else {}` (add and
  delete remote, mount, unmount, sync, save job, save config), two more
  (`run`/`delete` job) only flipped a badge without a word, and every loader
  dropped its error on the floor — so a refusal looked like a click that did
  nothing, and a failed refresh looked like an empty list. All of them now go
  through `showFailure()`. A failed sync-jobs refresh keeps the jobs already on
  screen instead of replacing them with "no jobs".
- **Sync selects offer every configured remote**, including `local` type ones.
  The old `cloudRemotes` filter hid them although `build_rclone_path`
  supports them — a screen-only restriction with no server rule behind it.
- The **job status badge reads the server-side run state** (`last_status`,
  `last_run_at`, `last_message`) and the schedule columns (`schedule_kind`,
  `next_run_at`) — ROADMAP 16.4 closed the debt this bullet used to describe.
  The screen must render what the API returns, never the outcome of its own
  click.
- `frontend/src/lib/services/api.ts`: `getRcloneStatus`, `getCloudRemotes`,
  `getCloudProviders`, `getCloudMounts`, `addCloudRemote`, `deleteCloudRemote`,
  `mountCloudRemote`, `unmountCloudRemote`, `syncCloud`, `copyCloud`,
  `getCloudConfig`, `saveCloudConfig`, `listSyncJobs`, `createSyncJob`,
  `deleteSyncJob`, `runSyncJob`, `updateSyncJobSchedule`. `unmountCloudRemote(mountPoint)` sends
  `{ mount_point }` — which is why the backend has a dedicated
  `MountStopRequest`.

## Confirmed bugs found and fixed

1. **No auth anywhere.** Every handler took only `State(_state)`, no session
   check at all — `GET /rclone/config` returned the raw config file to anyone
   who could reach the server. rclone "obscures" secrets with a
   trivially-reversible encoding (not real encryption), so this was a
   plaintext credential leak for every configured remote (S3 keys, OAuth
   tokens, FTP/SFTP passwords). `sync`/`copy` accept a `"local"` path type
   with no `PathGuard`-style restriction, so combined with the missing auth
   this was also an unauthenticated arbitrary-file-read-and-write primitive
   (e.g. sync `local:/etc/shadow` out to a self-added remote). Fixed: reads
   use `check_session`, mutations use `check_admin`.
2. **The mount feature was completely broken.** `MountRequest`'s
   `read_only`/`allow_other`/`daemon` fields had no `#[serde(default)]`, but
   the frontend only ever sends `{ remote, mount_point, path }` — every real
   "Start Mount" click failed Axum's JSON deserialization before the handler
   ran. The unmount side had the same defect in reverse: `/mount/stop` required
   the same body, so the `{ mount_point }` the screen sends was a **422** and
   no unmount ever ran. Fixed with `#[serde(default)]` on the three bools and a
   dedicated `MountStopRequest`.
3. **`handle_add_remote` config-injection.** `req.config` (free text) was
   appended verbatim into the INI-style config with no validation — a value
   containing an embedded `\n[othersection]` line could silently inject or
   override an unrelated remote's section. Fixed: any `config` value with a
   line starting with `[` (after trim) is rejected, and the provider type is
   validated as a slug.
4. **Dead `handle_crypt` stub** — removed (see above).
5. **Misapplied remote-name dropdown for local paths** in
   `CloudSync.svelte` — replaced with `FolderPicker` (see Frontend above).
6. **A mount could not report its own failure** (16.2). `spawn()` + `--daemon`
   meant rclone forked and every outcome — bad token, missing `/dev/fuse`, a
   mount that never appeared — looked like success. `shutdown_signal` did not
   know about mounts at all, so a service restart left stale FUSE mounts.
7. **No deadline on any rclone call** (16.1). A stalled probe, mount, transfer
   or unmount held its handler task forever; the `cloud_sync` activity job
   never closed.
8. **Credentials on disk were world-readable** (16.3): `tokio::fs::write`
   created the config 0644 (umask) and `tokio::fs::copy` recreated the backup
   with the source's mode.

## Invariants

- Read endpoints require a valid session (`check_session`); anything that
  can start a transfer, mount a filesystem, or read/write the raw config
  requires an admin session (`check_admin`).
- **Every rclone subprocess is bounded** by one of the four deadlines above,
  through `run_timed_async_result`. Do not add a bare `.output()`/`.status()`.
- **A job's run state is written by the server, never by the client.** Both the
  "Run now" button and the scheduler go through `run_saved_job`, which calls
  `record_run`; a screen that renders `last_status` must read it from the API,
  not remember its own response (ROADMAP 16.4).
- **A scheduled job never gains `--delete`.** `request_for` builds the request
  from the stored definition with `delete: false`, `dry_run: false`,
  `update: true` — a scheduled run must be no more destructive than a
  hand-started one.
- **One run per job at a time.** `JobRunGuard` is process-wide; without it the
  tick and a manual click would run two `rclone` processes over the same pair of
  directories.
- **A saved job with `schedule_kind = 'none'` is never due.** The due query
  filters on the kind as well as on `next_run_at`, so clearing a cadence really
  stops the runs rather than leaving one final firing behind.
- **A mount this process starts is a mount this process stops.** The child
  lives in `MOUNTS` until `handle_mount_stop`/`shutdown_mounts` reaps it; never
  reintroduce `--daemon`, `kill_on_drop(true)` or a dropped `Child`.
- **A mount point is valid only if `validate_mount_point` accepts it**: an
  absolute path that is a strict child of `RCLONE_MOUNT_BASE`, no `.`/`..`, no
  control characters, no leading `-` folder.
- `add_remote` rejects any `config` value containing a line starting with
  `[`, any provider type that is not a slug, and any remote name that is not a
  plain name — all three are written into the INI file verbatim.
- **The config and its backups are written 0600**, through
  `write_private_config`. `CONFIG_FILE_MODE` is the only place the mode is
  spelled.
- **Section edits are parsed**: use `config_has_section`/`remove_section`,
  never `contains("[name]")` or a byte search for the next `[`.
- **One config path, resolved in one place.** `rclone_config_path()`
  (`src/handlers/rclone.rs`) is the **only** way this module obtains the file it
  reads *and* writes: an explicit non-empty `RCLONE_CONFIG` wins outright
  (existing or not — that is how a config is created outside `/etc`), else the
  first of rclone's locations that exists (`/etc/rclone.conf`,
  `/root/.config/rclone/rclone.conf`, `$HOME/.config/rclone/rclone.conf`), else
  `/etc/rclone.conf`. `read_rclone_config`/`load_remotes` go through it too,
  instead of looping over the candidates on their own. **Do not reintroduce a
  hardcoded `/etc/rclone.conf` in a handler**: `handle_get_config`,
  `_save_config`, `_add_remote` and `_delete_remote` all used to, so the
  Settings editor read and wrote a different file than the one the remote list
  was parsed from — an admin who moved the config (the supported way to run
  rclone as a non-root user) got an empty list beside unrelated editor
  contents, and saving failed with `Permission denied` (against a root-owned
  `/etc`) for anyone but root. Guarded by
  `handlers::rclone::tests::test_parse_and_the_handlers_resolve_the_same_file`
  and, over HTTP, by
  `tests/rclone_integration.rs::config_endpoints_use_the_rclone_config_override`,
  `save_config_writes_the_overridden_file_and_keeps_a_backup` and
  `add_list_delete_remote_round_trip_on_the_overridden_file`.
- `handle_sync` / `handle_copy` register an **indeterminate** `cloud_sync`
  `PROGRESS_MANAGER` job around the blocking `rclone` call so the transfer
  shows in the desktop [activity taskbar](./activity.md); completed on
  success, failed with the reason on a non-zero exit **or** a timeout, on every
  return branch. (Determinate `--stats` parsing would need converting the
  handler to `spawn` + incremental stderr reads — a follow-up.)
- **Verified live** (isolated dev server, a scratch `RCLONE_CONFIG` pointing
  at a fake `[testremote] type = local` config, real `curl` calls, not
  mocks): every endpoint now rejects a request with no session token;
  `add_remote` rejects a `config` value containing an embedded
  `[othersection]` line. The 2026-10-23 pass is a code-level verification
  only: no mount or transfer was started on this host, so the mount-ready
  poll, the shutdown unmount and the 0600 repair are covered by unit tests
  (validators, `remove_section`, `/proc/mounts` matching, deadline ordering)
  and by integration tests over HTTP (mode bits on a tempdir config, refusals,
  the `{ mount_point }` body), not by a live FUSE mount.

## Scheduling a persisted job (ROADMAP 16.4, 2026-10-24)

Before this, `POST /rclone/jobs/{id}/run` was the only way a saved job ever ran:
the `sync_jobs` row held the definition and nothing else, so a nightly sync
could not be expressed and the screen's "last run / status" columns were the
outcome of the click that had just happened. Both halves are now server-side.

### The cadence

`sqlx/migrations/059_sync_job_schedule.sql` adds, all nullable with `none` as
the default — an existing appliance behaves exactly as before until an admin
opts in:

| Column | Meaning |
|---|---|
| `schedule_kind` | `none` \| `interval` \| `cron` |
| `interval_minutes` | cadence for `interval` (1 … 366 days, inclusive) |
| `cron_expr` | 5-field cron for `cron`, evaluated in the **appliance timezone** (same `next_run_at` machinery as a Scheduled Task: `restic_scheduler::parse_schedule`) |
| `next_run_at` | when the scheduler will consider the job due (indexed) |
| `last_run_at` / `last_status` / `last_message` | the outcome of the last **finished** attempt |

Validation happens **before** the row is written (`NewSyncJob::validate_schedule`)
and names the field: an interval with no value or out of range, a cron that
cannot be parsed, a cron over 120 chars. A job whose cadence cannot be computed
must never exist, or the scheduler would pick it up on every tick — the failure
mode item 12.3 had to fix on the tasks screen.

### Due-time arithmetic

- **First run**: an interval job is due **one interval after creation** (an
  "every 24 h" job created at 09:00 first runs tomorrow at 09:00 — not
  immediately, which would surprise the admin still typing), a cron job at its
  first occurrence.
- **After a run**: an interval job is rescheduled from **the moment the run
  finished**, not from the previous due time — a sync that took twenty minutes
  would otherwise fire again immediately and the overlap guard would skip it,
  making "every hour" behave like "whenever the previous one ended". A cron job
  is rescheduled from its own schedule, which is what a cron means.
- Switching the cadence to `none` clears `next_run_at` (the scheduler stops
  picking it up) and **keeps** the last-run state — that is history.

### The loop, and why the transfer is no longer request-shaped

`rclone::run_scheduler_loop(state)` is spawned at startup
(`src/web/mod.rs`). Every `SYNC_SCHEDULER_POLL_SECS` (60 s) it asks
`SyncJobRepository::due_now(now)` (one indexed query) and, for each due job,
**spawns** the run and moves on. The transfer therefore happens outside any
HTTP request — this is the request-shaped-transfer debt paid off.

Two entry points can start a run (the button and the tick), so both take a
`JobRunGuard` — a process-wide `HashSet<i64>` for the same reason
`scheduled_tasks_scheduler::RUNNING` exists: a static is the only way two entry
points cannot each believe they own the job. A job that is still running is
skipped with its `next_run_at` untouched, so the claim is retried next tick.

### One core, one state write

`run_sync_core` (extracted from `handle_sync`) returns a plain `SyncOutcome`
instead of an `axum::Response`, precisely so the scheduler can call it. Both
paths go through `run_saved_job`, which runs the core and then writes
`record_run(status, message, finished_at, next_run)`: the server always owns the
state, whichever path started the run. A run whose state could not be saved logs
a `warn` — the screen would otherwise show the previous outcome as if it were
this one.

`POST /rclone/jobs/{id}/run` still awaits the run in the request: that is what
"Run now" means (the admin is watching it). It answers with the **updated job**,
so the screen renders what the server recorded rather than assuming `200` meant
"it worked".

## Related

- [Explorer](./explorer.md), [Docker](./docker.md), [API Contract](../API.md)
