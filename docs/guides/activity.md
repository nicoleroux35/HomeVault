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

## Not yet covered (phase 2)

Syncthing folder sync and Storage operations (mdadm resync, `zpool`/`btrfs`
scrub, `luksFormat`, RAID rebuild) — these need a **poller** that turns live
system state (`/proc/mdstat`, `zpool status`, the Syncthing API) into synthetic
jobs, plus live testing against real hardware, so they are a separate change.

## Tests

- Logic: `src/sse/progress.rs` `#[cfg(test)]`, `src/handlers/activity.rs`
  `#[cfg(test)]`.
- Integration: `tests/activity_integration.rs`.
- E2E: `tests/e2e/desktop/desktop.spec.ts` section 13 (13.9 backoff, 13.10 a
  failed row keeps its outcome and no badge).

## Related docs

- [Architecture](../ARCHITECTURE.md) (SSE dispatch), [API Contract](../API.md),
  [Notifications](./notifications.md), [Scheduled Tasks](./scheduled-tasks.md),
  [Backup / Restic](./backup-restic.md).
