# Disk Cache Tier (HomeVault write caching)

**Purpose:** A designated fast "cache" pool absorbs writes first; a periodic
**mover** drains data to a slower "array" (target pool) per share, according to
each share's cache mode. Management lives in the **Storage** app (Disks): the **Pools** tab creates
/ designates / removes the cache disk, and the **Cache** tab configures the
mover and per-share cache modes.

## Backend

Migration `sqlx/migrations/016_cache.sql` adds three tables:

- `cache_settings` — singleton row (`id = 1`): `cache_pool` (TEXT, nullable),
  `mover_cron` (TEXT, default `0 3 * * *`), `mover_enabled` (INTEGER bool,
  default 0), `min_free_percent` (INTEGER, default 20), `mover_last_run_at`,
  `mover_age_days` (INTEGER, default 0), `mover_excludes` (TEXT, nullable),
  `mover_bwlimit` (INTEGER KB/s, default 0).
- `cache_shares` — one row per cache-enabled dataset: `id`, `name`,
  `cache_mode`, `target_pool`, `enabled`, `created_at`, `updated_at`.
- `cache_mover_runs` — mover run history: `id`, `status`, `started_at`,
  `finished_at`, `files_moved`, `bytes_moved`, `error_message`.
- `cache_mover_run_shares` (migration
  `sqlx/migrations/052_cache_mover_run_shares.sql`) — one row per enabled share
  per run: `run_id`, `share_name`, `mode`, `status`
  (`moved | skipped | failed`), `files_moved`, `bytes_moved`, `message`,
  primary key `(run_id, share_name)`. A run row alone cannot say *which* shares
  are now split across the two tiers.

`sqlx/migrations/017_cache_mover_tuning.sql` adds the three `mover_*` tuning
columns to `cache_settings`.

Storage layer `src/storage/cache_repo.rs` (`CacheRepository`): `get_settings`,
`save_settings`, `set_mover_last_run`, `list_shares`, `get_share`,
`create_share`, `update_share`, `delete_share`, `list_runs` (which loads the
per-share rows of exactly the returned runs in one extra query — no N+1),
`create_run`, `update_run`, `record_run_shares`, `fail_stale_runs`. Pool mounts
are resolved once per run/request through `PoolMounts::scan` (one
`PoolService::scan_pools` call, `src/pools.rs`), exactly like the rest of the
Storage area.

Core logic `src/cache.rs` (`CacheService`):

- `CacheMode` enum — `Yes | No | Prefer | Only`, serialized as
  `yes | no | prefer | only`.
- `PoolMounts` — pool name → mount point, built from **one** pool scan per
  mover run or per request. `PoolMounts::scan()` returning `Err` is a *scan*
  failure, which is a different condition from "this pool does not exist"
  (a missing key in the map); handlers must not collapse the two.
- `active_path` returns the live location of a share's data given its
  mode (cache mount vs. target mount).
- `perform_moves` walks every enabled share and applies the mode's
  transfer rule with `rsync -a --stats --remove-source-files`, plus the mover
  tuning options: `--exclude=<pat>` for each entry in `mover_excludes`
  (comma/space separated), `--bwlimit=<kbps>` when `mover_bwlimit > 0`, and an
   age filter (files older than `mover_age_days` days, via `find -mtime` +
   `rsync --files-from`) so hot/recent files can stay on the cache. `rsync -a`
   verifies every block and `--remove-source-files` only removes files it
   successfully transferred; exit code 24 ("vanished source files" — a benign
   race where a file was deleted after rsync built its list) is treated as
   success so it doesn't fail an otherwise-complete run.

   **Free-space gate (before any delete).** Because `--remove-source-files`
   deletes each file once it has been copied, a destructive move first proves
   the destination can hold the payload: `ensure_destination_has_room` sums the
   apparent size of the files that would move (`find -type f -mtime … -printf
   '%s\n'`, the same age filter `rsync` uses) and compares it with the free
   bytes on `dst` (`df -B1`). If the destination cannot hold it — or `df` cannot
   be read at all — the share fails with an explicit message and **rsync never
   runs**, so nothing is deleted. `mover_excludes` is deliberately *not*
   subtracted: over-estimating the payload is the safe direction. With an age
   filter that selects nothing, the rsync round-trip is skipped entirely.

   Mode semantics (in `move_share`, called per share by `perform_moves`):
  - `No` — data lives on the array; anything left on the cache is moved back
    to the array.
  - `Yes` — write to cache; moved to the array and the cache copy is removed.
  - `Only` — keep permanently on cache; anything on the array is moved back to
    the cache.
  - `Prefer` — keep on cache, but once free space on the cache drops to/below
    `min_free_percent` (i.e. `pool_usage_pct(cache) >= 100 - min_free_percent`,
    threshold read via `df -B1`), move to the array.
- `perform_moves` never aborts on a share: it returns a `MoverResult`
  (`files_moved`, `bytes_moved`, `shares_processed`, `shares: Vec<ShareOutcome>`).
  A share that cannot be moved becomes a `Failed` outcome with a message and the
  run continues with the next share. `MoverResult::status()` derives the run
  status: `completed` (nothing failed), `partial` (some moved, some failed),
  `failed` (every attempted share failed). `failed_shares()` names them for the
  run summary.
- `run_mover` is the async entry point: claims the process-local single-flight
  guard, closes out any `running` row left by a previous process
  (`fail_stale_runs` — holding the guard proves nothing is in flight), loads
  settings + shares, records a `cache_mover_runs` row, runs **one** pool scan
  plus `perform_moves` in a blocking task (emitting live SSE progress per share
  when a `job_id` is supplied), persists the per-share outcomes
  (`record_run_shares`), updates the row to `completed`/`partial`/`failed`,
  re-points every enabled cache share's SMB export to its new active path
  (`refresh_smb_exports`, so network clients keep seeing the data after a move),
  records a system notification (`notify_mover` → `NotificationRepository`,
  category `cache`, naming the shares that were not moved), and marks the SSE
  job done. It never leaves a run stuck on `running`.

Scheduler `src/cache_scheduler.rs` (`run_scheduler_loop`): polls every 30s;
when the next cron tick is due **and** `mover_enabled`, it invokes
`run_mover`. Only a successful run advances `mover_last_run_at`: a failed one is
retried, not skipped until the next cron occurrence. To keep that retry from
spinning every 30 s, a process-local failure timestamp (`LAST_FAILED_AT`) holds
the next attempt off for `FAILED_RETRY_SECS` (15 min); a restart clears it.
Spawned once at startup in `src/web/mod.rs`.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /cache/settings` | auth | current cache-tier config |
| `PUT /cache/settings` | admin | set `cache_pool`, cron, enabled, min_free |
| `GET /cache/pools` | auth | pools usable as cache/array tiers |
| `GET /cache/shares` | auth | cache shares |
| `POST /cache/shares` | admin | create share + matching SMB export |
| `PUT /cache/shares/{name}` | admin | change mode / target / enabled |
| `DELETE /cache/shares/{name}` | admin | remove share (data left on disk) |
| `POST /cache/mover/run` | admin | run the mover manually now |
| `GET /cache/mover/runs` | auth | mover run history |

All handlers in `src/handlers/cache.rs` (`handle_cache_settings_get`,
`handle_cache_settings_update`, `handle_cache_pools_get`,
`handle_cache_shares_list`, `handle_cache_share_create`,
`handle_cache_share_update`, `handle_cache_share_delete`,
`handle_cache_mover_run`, `handle_cache_mover_runs`). Routes are registered
in `src/web/mod.rs`. Read endpoints use `require_auth`; mutations use
`require_admin`. **Do not** revert any handler to a bare "header present"
check.

## Deadlines on `rsync` / `find` / `df` (2026-10-24, ROADMAP X.2)

The mover's three host tools used to run unbounded, from inside
`spawn_blocking`. The consequence was worse than one stalled job: `run_mover`
holds the **process-wide** `MOVER_RUNNING` slot for the whole run, so a wedged
`rsync` kept that slot set and **no mover ran again** — not on schedule, not
from the button — until the service was restarted.

Every call now goes through `crate::utils::process::run_timed` via one local
`run_command(cmd, timeout)` helper:

| Call | Deadline | Why |
|---|---|---|
| `df -B1 <path>` | `CACHE_QUERY_TIMEOUT` 30 s | mount-table read; also gates the free-space check that must answer *before* `rsync --remove-source-files` deletes anything |
| `find … -printf` (sizing, age list) | `CACHE_FIND_TIMEOUT` 600 s | proportional to the file count on the cache tier |
| `rsync -a --stats …` | `CACHE_RSYNC_TIMEOUT` 6 h | a first drain of a full tier legitimately takes hours — a **ceiling**, like rclone's transfer budget |

The child is killed at the deadline (`kill_on_drop`), so the run ends, the
`MoverRunGuard` releases the slot on drop, and the run row is recorded as
failed with the deadline in its message. `run_command` keeps a non-zero exit as
*data*, which the `rsync` path depends on: exit 24 means "vanished source
files" and is treated as success for the files that did transfer.

## Frontend

- `frontend/src/routes/Storage.svelte`:
   - **Pools** tab — cache-disk management (the "Disks" entry point the user
     asked for): "Create Cache Disk" button (`openCreateCacheDiskModal`)
     opens the pool-create modal pre-checked as cache; each pool card
     shows a **CACHE** badge when it is the active cache disk and a
     set/unset button (`setPoolAsCache`, `unsetPoolAsCache`);
     deleting the cache pool clears the designation (`confirmDeletePool`,
     with an explicit warning in the delete modal).
   - **Cache** tab — config card (`saveCacheSettings`), mover control
     (`runMover`, with live SSE progress bar), shares table + create
     modal (`openCacheShareModal`, `createCacheShare`,
     `updateShareMode`, `deleteCacheShare`). The run history shows one row per
     run (status badge `completed | partial | failed | running`, date, files and
     bytes) with the per-share outcome underneath — each share's name, its
     `moved | skipped | failed` status and, on failure, the reason (e.g. the
     destination's free space).
   - `loadCachePoolDesignation` keeps the Pools-tab badge in sync
     without loading the full cache tab.
- `frontend/src/lib/services/api.ts`:
  `getCacheSettings`, `updateCacheSettings`,
  `getCachePools`, `getCacheShares`, `createCacheShare`,
  `updateCacheShare`, `deleteCacheShare`,
  `runCacheMover` (returns a `job_id` for live SSE progress via
  `subscribeToProgress`), `getCacheMoverRuns`. Types
  `CacheSettings` (now includes `mover_age_days`, `mover_excludes`,
  `mover_bwlimit`), `CachePoolInfo`, `CacheShare`, `CacheMoverRun` (with
  `shares: CacheMoverRunShare[]`).

## i18n

All strings under `[cache]` in `src/i18n/{en,fr,de,es}.toml`. Key groups:
cache-tab chrome (`tab`, `title`, `subtitle`), configuration (`cache_pool_*`,
`mover_*`, `min_free_*`, `mover_age_label`, `mover_excludes_label`,
`mover_bwlimit_label`, `save_config`), mover (`run_mover`, `mover_started`,
`mover_hint`), run history (`run_shares`, `run_status_*`,
`share_status_*`), shares (`add_share`, `share_name`, `mode`, `target_pool`,
`mode_yes/no/prefer/only`), and cache-disk management in the Pools tab
(`create_cache_disk`, `set_as_cache`, `unset_cache`, `badge`, `is_cache_hint`,
`as_cache_option`, `delete_is_cache`, `created_as_cache`, `set_failed`,
`removed_cache`, `remove_failed`).

The mover runs live over SSE (`JobType::CacheMover`, surfaced in the UI via
`api.subscribeToProgress`) and posts a system notification (category `cache`)
on completion/failure, reusing the shared `NotificationRepository`.

## Invariants

- `cache_pool` in `cache_settings` is just a **pool name** string — there is no
  separate "cache disk" entity. Creating/removing the cache disk is
  create/delete of a normal pool plus setting/clearing this field. The Pools
  tab is the only supported place to create or remove it.
- `updateCacheSettings({ cache_pool: null })` **and** `cache_pool: ""` (or
  whitespace) both clear the designation; omitting the field leaves it
  untouched (`normalize_pool_field`, `src/handlers/cache.rs`). An empty string
  used to become an empty pool name that then failed validation, so only
  `null` could clear it. The frontend sends `null`.
- `min_free_percent` is validated server-side to **1–95** (the UI has always
  clamped it that way); `0` would mean "drain the cache to nothing" for
  `Prefer` shares.
- The handlers validate the cache pool and each share's `target_pool`
  (`validate_target_pool`: non-empty, exists, `!= cache_pool`) on **both**
  create and update. The update path used to accept any string, leaving a share
  the mover could never move. Pool scans go through `scan_pools_async`
  (`tokio::task::spawn_blocking`): the scan shells out to `zpool`/`mdadm`/
  `btrfs`/`df`, so running it inline parked a runtime worker, and the old
  `unwrap_or_default()` reported a failed scan as "that pool does not exist".
- The mover shells out to `rsync`, `find` and `df`, so it is CPU/IO-bound — run
  it off the request path (`run_mover` is the scheduler/endpoint target, never
  inline in a hot path). **Each of those calls carries a deadline** (see
  "Deadlines" above): the single-flight slot is process-wide, so an unbounded
  tool would not just stall one run — it would block every later run until a
  restart. Never call `Command::output()` directly in this module; a source
  guard (`test_no_mover_subprocess_runs_without_a_deadline`) fails the build if
  one comes back.
- Creating a cache share also creates/updates a matching SMB export
  (`create_or_update_smb_export`, `src/handlers/cache.rs`) so the data is
  reachable over the network at its active path. After a mover run,
  `refresh_smb_exports` (`src/cache.rs`) re-points those exports to the
  share's new active location.
- The scheduler and every handler keep the same auth split as the rest of the
  Storage area (auth for reads, admin for mutations). Do not weaken it.

## Verified

- `cargo build` and `npm run build` both pass.
- `svelte-check` is clean for the cache code (remaining `Storage.svelte`
  warnings are the pre-existing lucide icon `Component` typing false positives
  that affect every tab icon across the app).
- Backend `cargo check` passes. The mover's data-movement path (`perform_moves`
  → `move_share` → `rsync_move`) **is** exercised by unit tests against real
  temporary directories with real `rsync`/`find`/`df`: a normal move, an empty
  share (skipped), an unmounted target (failed, source untouched), a failing
  share followed by a succeeding one (partial), and a destination too small for
  the payload (refused, nothing deleted, nothing written). `tests/cache_integration.rs`
  covers the API validation, the `cache_pool` clearing path, the per-share
  history round-trip and the stale-`running` reconciliation. Cron scheduling and
  the create/designate/remove-cache-disk flows in the Pools tab were still not
  run against real pools/hardware.

## Related

- [Storage](./storage.md), [Shares](./shares.md), [Snapshots](./snapshots.md)
- Backend: `src/cache.rs`, `src/cache_scheduler.rs`, `src/storage/cache_repo.rs`,
  `src/handlers/cache.rs`
