# Activity Taskbar (unified background-job feed)

**Purpose:** One place on the desktop that shows what the server is doing —
downloads, file copies, backups, scans, app installs, disk clones, model
downloads, cloud syncs — as a dock button (badge = count) that opens a panel of
progress rows above the dock.

## Backend

### `GET /api/activity` — `require_auth`

`src/handlers/activity.rs::handle_activity`. Merges two sources into one list:

1. **`PROGRESS_MANAGER` jobs** (`src/sse/progress.rs`) that are `pending` or
   `running`, **plus** a job that `failed` or was `cancelled` within the last
   minute (`FAILED_GRACE`) — see *A failure stays visible* below.
2. **aria2 active downloads** — only when the daemon is already up
   (`downloader::aria2_is_running`, which reaps a dead child through
   `try_wait` instead of trusting a stale handle), and only for admins.
   Read-only `aria2.tellActive` through the shared secret-aware RPC helper
   (`downloader::aria2_active_tasks`, 1.5 s timeout); **never** starts aria2.

**Owner scoping:** a non-admin sees only jobs whose `owner` equals their
username; an admin sees every job. Jobs with `owner = None` are
system/admin-triggered and are admin-only. This closes the cross-user path-leak
noted in `src/handlers/progress.rs`.

Response — `ApiResponse<ActivityTask[]>`:

| Field | Type | Notes |
|-------|------|-------|
| `id` | string | `PROGRESS_MANAGER` uuid, or `"aria2:<gid>"` |
| `kind` | string | stable slug, localised on the client (`desktop.activity_kind_<kind>`) |
| `state` | string | `pending` \| `running` \| `failed` \| `cancelled` — the row's lifecycle, not a label |
| `label` | string | dynamic name (torrent / file); `""` for a generic job |
| `progress` | number \| null | `0..100`; `null` = indeterminate (no `%` reported yet) |
| `detail` | string \| null | latest message / `"12/40 files"` / `"8.4 MB/s"` |

### `kind` slugs

| Slug | Source |
|------|--------|
| `backup` / `restore` / `verify` | Restic (`src/handlers/restic.rs`) |
| `cache_mover` | Disk cache mover (`src/handlers/cache.rs`, `src/cache.rs`) |
| `antivirus_scan` | ClamAV scan (`src/antivirus.rs`) |
| `explorer_copy` / `explorer_move` | File Explorer directory copy/move (`src/handlers/explorer.rs`) — carries an `owner` |
| `docker_install` / `docker_update` | Docker catalog install/update (`src/handlers/docker_catalog.rs`) — indeterminate |
| `rescue_ddrescue` / `rescue_photorec` | Rescue disk clone / file recovery (`src/handlers/rescue.rs`) — carries an `owner` |
| `rag_index` | Knowledge-base reindex (`src/handlers/rag.rs`) — indeterminate |
| `ollama_pull` | AI model download (`src/handlers/chat.rs`) — **determinate** (streamed `completed`/`total`) |
| `cloud_sync` | rclone sync/copy (`src/handlers/rclone.rs`) — indeterminate |
| `download_torrent` / `download_url` | aria2 active download — **determinate** when the total size is known |
| `raid_resync` | mdadm resync / recovery / reshape / consistency check, read from `/proc/mdstat` (`src/activity_poller.rs`) — **synthetic**, ownerless (admin-only) |
| `pool_scrub` | ZFS `zpool status` scrub/resilver or `btrfs scrub status` (`src/activity_poller.rs`) — **synthetic**, ownerless |
| `syncthing_sync` | Syncthing folder still catching up (`src/activity_poller.rs`) — **synthetic**, ownerless |

### `ProgressManager` changes (`src/sse/progress.rs`)

- `JobInfo` retains the last `ProgressEvent` (`last_event`), a `has_progress`
  flag (tells `0 %` apart from "nothing reported yet"), and an `owner`.
- `create_job_with_owner(job_type, msg, owner)` — `create_job` delegates to it
  with `owner = None` (signature unchanged).
- `list_activity() -> Vec<ActivitySnapshot>` — the feed's data source.
- `get_progress(job_id)` returns the job's retained `last_event` (or `None` for
  an unknown/removed id), so `GET /api/progress/{job_id}/status` agrees with the
  feed and the SSE stream. It used to synthesise a fresh `Running` / 0 % /
  `"Waiting..."` event for any tracked id (fixed 2026-10-12).
- `emit_message_only(job_id, msg)` — advances the retained message **without**
  claiming a percentage (`has_progress` untouched). `emit_progress` always sets
  that flag, which is what tells `0 %` apart from "nothing reported yet"; the
  activity poller (ROADMAP 28.3) needs to keep advancing a no-number source's
  *message* while its bar stays indeterminate, so it goes through this instead
  — otherwise an honest `progress: null` became a fake `0 %` that never moved.
- `JobType` is serde-**adjacently** tagged (`tag = "type"`, `content = "name"`):
  `{"type":"backup"}` for the built-in kinds, `{"type":"custom","name":
  "explorer_copy"}` for the feature slugs in the table above. Internal tagging
  alone cannot serialize the `Custom` newtype variant, which emptied every SSE
  payload for a feature job (fixed 2026-10-12 — see
  [Explorer](./explorer.md) → Progress reporting). This feed is unaffected
  either way: `job_type_kind` maps the enum to the slug client-side.
- `cleanup_old_jobs` now keeps a job while its *live* state is `running`
  (previously it checked the never-updated create-time seed).

### A failure stays visible (2026-10-24, ROADMAP 28.2)

The feed used to keep only `pending | running`, so a job that failed **one
second ago** disappeared on the very next poll: the row the user was watching
simply vanished, and the only trace left was a bell notification they were not
looking at. A cancelled job had the same problem.

Now a job whose live state is `failed` or `cancelled` stays for
`FAILED_GRACE` (60 s) after its **last event** (`ActivitySnapshot.updated_at`,
taken from `ProgressEvent.timestamp`), then drops out. A `completed` job is
still never shown — the screen that started it reports its own success, and the
feed is about work in progress.

The row carries `state`, so the client can render a failure as a failure: no
`role="progressbar"` (there is no progress to report), the localised word
instead, and it does **not** count towards the dock badge — nothing is running
any more. `is_visible()` / `state_slug()` are pure and unit-tested, including
the "no timestamp ⇒ not shown" case (a terminal state with no evidence of
*when* would otherwise stay forever).

## Live-hardware poller (ROADMAP 28.3, phase 2)

Until 2026-10-24 the feed could only show work **HomeVault started**: a job is a
`PROGRESS_MANAGER` entry some handler created. Everything the *host* does on its
own — a Syncthing folder finishing a large scan, an mdadm array resyncing, a
`zpool scrub`, a btrfs scrub — was invisible, even though it is exactly the kind
of long, slow, "is it stuck?" work the taskbar exists for.

`src/activity_poller.rs` closes that gap. `web::run()` spawns
`run_poller_loop(state)` at startup with `state.clone()`; every `POLL_INTERVAL`
(5 s) it reads the enabled sources and **synthesises** a `PROGRESS_MANAGER` job
per live operation.

### Sources

| Source | Read | Live when | `kind` |
|---|---|---|---|
| mdadm | `/proc/mdstat` (`tokio::fs`, no privilege) | a `mdN` array carries a `[==>…]` status line with `resync` / `recovery` / `reshape` / `check` and is not `DELAYED`/`PENDING` | `raid_resync` |
| ZFS | `sudo zpool status <pool>` | the `scan:` block says `scrub in progress` or `resilver in progress` | `pool_scrub` |
| btrfs | `sudo btrfs scrub status <mount>` | `Status: running` | `pool_scrub` |
| Syncthing | `GET /rest/system/config` + `GET /rest/db/status?folder=…` per folder, through `handlers::syncthing::folder_backlogs` | `needBytes > 0` and the folder is not paused | `syncthing_sync` |

Every subprocess is bounded (`ZPOOL_PROBE_TIMEOUT` / `BTRFS_PROBE_TIMEOUT`, 10 s
each, `run_timed_async` — ROADMAP X.9), and the module carries its own
`every_subprocess_is_bounded` source guard.

### One row per operation, not one per poll

The job id is a **deterministic function of the source** — `sys:raid:md0:resync`,
`sys:pool:tank:scrub`, `sys:syncthing:<folder-id>` — so a poller tick that sees
the same operation again **advances** the same job instead of creating a new
one. This is the whole design: an id that changed every tick would fill the
panel with a ghost row per five seconds and no bar would ever move.

### When an operation ends

`plan_reconcile(known, live)` computes, purely, which ids to create/advance and
which to drop: `finished` is `known − live`, and it keeps **only** ids carrying
the `sys:` prefix (`SYNTHETIC_PREFIX`). The poller therefore can never complete
a job another handler created — a real backup named `md0` is not a resync. A
synthetic job discovered part-way through (the manager's cleanup pruned it) is
re-created at its real percentage, not at 0 %.

At startup the loop seeds `known` from whatever synthetic ids the *previous*
process left in the manager (`synthetic_ids_in_manager`), so a restart does not
orphan a scrub row.

### Failure policy

A source that cannot be read is a **gap, never a fabricated value**:

- `/proc/mdstat` missing means "no md arrays on this host" — an empty list, not
  an error and not "healthy".
- a `zpool`/`btrfs` probe that times out or fails (tool absent) contributes
  nothing; other sources are unaffected.
- Syncthing is only probed when its `config.xml` exists
  (`syncthing_config_present`, on the blocking pool), so a box that never
  installed it does no HTTP work at all.
- the loop is unkillable: a failing tick is logged and the next one runs.

A probe failure is **never** turned into a red row: a failed measurement is not
new information about the array.

### Four traps worth knowing before touching this module

All four were shipped bugs caught by the item's own tests, not by re-reading
the code. They are documented because each one fails **silently** — the feed
still answers `200`, it just never shows the row.

1. **`create_job()` mints a uuid — use `create_job_with_id()`.** The whole
   design rests on the id being derived from the source. Creating the job with
   the uuid constructor inserts it under an id nothing addresses again, so
   every later `emit_progress(&op.id, …)` is a no-op and the row never exists.
   `apply_plan` carries a `debug_assert` on that id for exactly this reason.
2. **`/proc/mdstat` draws its progress bar with `=`.** `[==>……]  resync =
   12.6%` — so `split_once('=')` lands *inside the bar*. Look for the `=` after
   the closing `]`, which is what `parse_md_percent` does now.
3. **`zpool` punctuation.** The token is `29.37%` followed by `done,` — with the
   comma. Comparing against a literal `"done"` never matches; both tokens must
   be trimmed of `,`/`;` and `%` first.
4. **`emit_message_only` must keep the last percentage.** `list_activity`
   answers with `last_event.progress` once `has_progress` is set, so writing a
   bare `0.0` on a message-only tick drops a row that had reached 29 % back to
   "0 %" — the exact failure the method exists to prevent.

### Deliberate non-coverage

- `luksFormat` — one-shot, no progress to read. A row could only ever say
  "running" until it succeeded or failed, which the Storage screen already
  reports.
- The kernel's own `recovery` progress on a **non-md** device (e.g. a
  hardware RAID controller) — no portable source.

### Tests

The parsers are pure and fully unit-tested without hardware: `parse_mdstat` /
`parse_md_status_line` / `parse_md_percent` against an idle, resyncing,
delayed and inactive `/proc/mdstat`; `parse_zpool_scan` against a scrubbing, a
finished and a young (no estimate) pool; `parse_btrfs_scrub_status` against
running, finished and aborted; `folder_sync_progress` on the bytes-not-files
rule; and `plan_reconcile`/`apply_plan` for the one-row-per-operation rule. The
integration suite (`tests/activity_poller_integration.rs`) drives a real
`/proc/mdstat` fixture through `tick()` and reads the result back through
`GET /api/activity` — the contract the taskbar consumes.

## Frontend

- Store: `frontend/src/lib/stores/activityStore.ts` — a `readable` that polls
  `GET /api/activity` **only** while a component is subscribed (the desktop is
  mounted), the tab is visible, and the user is logged in. Exposes
  `{ tasks, count }` where `count` is the number of **active** rows
  (`pending`/`running`), not `tasks.length`: a recent failure is displayed but
  is not work in progress.
- **Polling backs off when the backend fails** (2026-10-24, ROADMAP 28.1).
  The store used `setInterval(poll, 2000)` unconditionally, so a backend
  answering `500` was hit **30 times a minute per open desktop** — a
  self-inflicted load spike exactly when the box is already unwell. The loop is
  now a `setTimeout` chain driven by `nextPollDelay(failures)`: 2 s while
  healthy, then 4, 8, 16, 32, **60 s** (cap, `POLL_MAX_MS`) and 60 s from then
  on. Two failures in a row (`FAILURE_TOLERANCE`) clear the list; one is
  tolerated so the panel does not flicker. Coming back to the tab resets the
  backoff and polls immediately, so a panel opened after a long sleep is not
  stale for a minute. No jitter — one appliance with a handful of desktops is
  not a fleet that needs de-synchronising, and a deterministic series is what
  the E2E assertion pins (≤3 requests in 7 s, against 4 for a 2 s cadence).
- `frontend/src/lib/services/api.ts`: `getActivity()`, `ActivityTask`.
- Desktop shell (`frontend/src/routes/Desktop.svelte` — this feature
  was an explicit request):
  - `$: activeJobCount = $activity.count` drives the existing thin
    `.hv-menubar-activity` strip on the topbar pill (the old 8 s
    `/api/progress/jobs` poll is gone).
  - Dock button `.hv-dock-icon-activity` renders while `activityRowCount > 0`
    (any row to report, a recent failure included); `.hv-dock-badge` renders
    only when `activityCount > 0`. `toggleActivityPanel()` opens
    `.hv-activity-panel`.
  - The panel is a solid paper-white card flush above the dock, `width` bound to the
    dock's `clientWidth`. One `.hv-activity-row` per task: name, `%`, and a
    `.hv-activity-track` progress bar (`.hv-activity-fill--indet` sliding
    gradient when `progress == null`). **Display-only — no row click handler.**
  - A `failed`/`cancelled` row is `.hv-activity-row--failed` with
    `data-state`: no progress bar (a 4px `--hv-error` rule instead), the
    localised outcome in `.hv-activity-pct` in `--hv-error-text` (9.4:1 on
    white). Colour is never the only signal — the word is always there.
  - Closes on Escape, outside click, a second button press, `openApp`, any
    topbar panel opening, when `activityRowCount` hits 0, and when the dock
    auto-hides.

## Invariants

- `require_auth` on `/api/activity`; the aria2 branch additionally requires
  admin and must never call `ensure_aria2_running`.
- `label` / `detail` never carry a secret. Non-admins never see another user's
  job (owner filter).
- The feed is polling, not SSE — deliberately matching the old
  `loadActiveJobs` cadence. A future `/api/activity/stream` could replace it
  (still open, see *Not yet covered* below).
- A failing backend is polled with **exponential backoff** (`nextPollDelay`,
  capped at `POLL_MAX_MS`); a 2 s interval on a `500` is not allowed to come
  back. Coming back to the tab resets it.
- A job that `failed`/`cancelled` stays for `FAILED_GRACE` and carries
  `state`; a `completed` one never appears. The badge counts active rows only.
- `activityStore` must not persist anything to `localStorage`.
- The poller's synthetic rows carry the `sys:` id prefix and no owner; the
  sweep only ever completes ids with that prefix (`plan_reconcile`), so a job
  another handler created is never touched.
- A poller source that cannot be read contributes **nothing**. A missing
  `/proc/mdstat`, a timed-out `zpool`, a Syncthing daemon that is not there —
  all are gaps, never a fabricated "healthy" and never a red row.
- The poller never spawns without a deadline and never blocks a worker on file
  I/O (`tokio::fs` for `/proc/mdstat`, `spawn_blocking` for the Syncthing path
  probe).

## Tests

- Logic: `src/sse/progress.rs` `#[cfg(test)]`, `src/handlers/activity.rs`
  `#[cfg(test)]`, `src/activity_poller.rs` `#[cfg(test)]` (the parsers and the
  reconciliation, no hardware).
- Integration: `tests/activity_integration.rs`,
  `tests/activity_poller_integration.rs` (a real `/proc/mdstat` fixture through
  `tick()` read back over `/api/activity`).
- E2E: `tests/e2e/desktop/desktop.spec.ts` section 13 (13.9 backoff, 13.10 a
  failed row keeps its outcome and no badge, 13.11 a synthetic `raid_resync`
  row renders and drops out when it disappears).

## Related docs

- [Architecture](../ARCHITECTURE.md) (SSE dispatch), [API Contract](../API.md),
  [Notifications](./notifications.md), [Scheduled Tasks](./scheduled-tasks.md),
  [Backup / Restic](./backup-restic.md).
