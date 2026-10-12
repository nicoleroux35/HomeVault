# Activity Taskbar (unified background-job feed)

**Purpose:** One place on the desktop that shows what the server is doing —
downloads, file copies, backups, scans, app installs, disk clones, model
downloads, cloud syncs — as a dock button (badge = count) that opens a panel of
progress rows above the dock.

## Backend

### `GET /api/activity` — `require_auth`

`src/handlers/activity.rs::handle_activity`. Merges two sources into one list:

1. **`PROGRESS_MANAGER` jobs** (`src/sse/progress.rs`) whose live state is
   `pending` or `running`.
2. **aria2 active downloads** — only when the daemon is already up
   (`state.downloader.lock().child.is_some()`), and only for admins. Read-only
   `aria2.tellActive` call with a 1.5 s timeout; **never** starts aria2.

**Owner scoping:** a non-admin sees only jobs whose `owner` equals their
username; an admin sees every job. Jobs with `owner = None` are
system/admin-triggered and are admin-only. This closes the cross-user path-leak
noted in `src/handlers/progress.rs`.

Response — `ApiResponse<ActivityTask[]>`:

| Field | Type | Notes |
|-------|------|-------|
| `id` | string | `PROGRESS_MANAGER` uuid, or `"aria2:<gid>"` |
| `kind` | string | stable slug, localised on the client (`desktop.activity_kind_<kind>`) |
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

## Frontend

- Store: `frontend/src/lib/stores/activityStore.ts` — a `readable` that polls
  `GET /api/activity` every 2 s **only** while a component is subscribed (the
  desktop is mounted), the tab is visible, and the user is logged in. Exposes
  `{ tasks, count }`.
- `frontend/src/lib/services/api.ts`: `getActivity()`, `ActivityTask`.
- Desktop shell (`frontend/src/routes/Desktop.svelte` — this feature
  was an explicit request):
  - `$: activeJobCount = $activity.count` drives the existing thin
    `.hv-menubar-activity` strip on the topbar pill (the old 8 s
    `/api/progress/jobs` poll is gone).
  - Dock button `.hv-dock-icon-activity` (+ `.hv-dock-badge`) renders only when
    `activityCount > 0`; `toggleActivityPanel()` opens `.hv-activity-panel`.
  - The panel is a solid paper-white card flush above the dock, `width` bound to the
    dock's `clientWidth`. One `.hv-activity-row` per task: name, `%`, and a
    `.hv-activity-track` progress bar (`.hv-activity-fill--indet` sliding
    gradient when `progress == null`). **Display-only — no row click handler.**
  - Closes on Escape, outside click, a second button press, `openApp`, any
    topbar panel opening, when `activityCount` hits 0, and when the dock
    auto-hides.

## Invariants

- `require_auth` on `/api/activity`; the aria2 branch additionally requires
  admin and must never call `ensure_aria2_running`.
- `label` / `detail` never carry a secret. Non-admins never see another user's
  job (owner filter).
- The feed is polling, not SSE — deliberately matching the old
  `loadActiveJobs` cadence. A future `/api/activity/stream` could replace it.
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
- E2E: `tests/e2e/desktop/desktop.spec.ts` section 13.
