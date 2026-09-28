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

`sqlx/migrations/017_cache_mover_tuning.sql` adds the three `mover_*` tuning
columns to `cache_settings`.

Storage layer `src/storage/cache_repo.rs` (`CacheRepository`): `get_settings`,
`save_settings`, `set_mover_last_run`, `list_shares`, `get_share`,
`create_share`, `update_share`, `delete_share`, `list_runs`, `create_run`,
`update_run`. Pool mounts are resolved through `PoolService::scan_pools`
(`src/pools.rs`), exactly like the rest of the Storage area.

Core logic `src/cache.rs` (`CacheService`):

- `CacheMode` enum — `Yes | No | Prefer | Only`, serialized as
  `yes | no | prefer | only`.
- `pool_mount_point` resolves a pool name to its real mount.
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
   success so it doesn't fail an otherwise-complete run. Mode
   semantics (in `perform_moves`):
  - `No` — data lives on the array; anything left on the cache is moved back
    to the array.
  - `Yes` — write to cache; moved to the array and the cache copy is removed.
  - `Only` — keep permanently on cache; anything on the array is moved back to
    the cache.
  - `Prefer` — keep on cache, but once free space on the cache drops to/below
    `min_free_percent` (i.e. `pool_usage_pct(cache) >= 100 - min_free_percent`,
    threshold read via `df -B1`), move to the array.
- `run_mover` is the async entry point: loads settings + shares,
  records a `cache_mover_runs` row, runs `perform_moves` (in a blocking task,
  emitting live SSE progress per share when a `job_id` is supplied), updates the
  row to `completed`/`failed` (and `set_mover_last_run`), re-points every
  enabled cache share's SMB export to its new active path
  (`refresh_smb_exports`, so network clients keep seeing the data after a move),
  records a system notification (`notify_mover` → `NotificationRepository`,
  category `cache`), and marks the SSE job done/failed. It never leaves a run
  stuck on `running`.

Scheduler `src/cache_scheduler.rs` (`run_scheduler_loop`): polls every 30s;
when the next cron tick is due **and** `mover_enabled`, it invokes
`run_mover`. Mirrors the Restic scheduler pattern — a failed run still
reschedules. Spawned once at startup in `src/web/mod.rs`.

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
     `updateShareMode`, `deleteCacheShare`).
   - `loadCachePoolDesignation` keeps the Pools-tab badge in sync
     without loading the full cache tab.
- `frontend/src/lib/services/api.ts`:
  `getCacheSettings`, `updateCacheSettings`,
  `getCachePools`, `getCacheShares`, `createCacheShare`,
  `updateCacheShare`, `deleteCacheShare`,
  `runCacheMover` (returns a `job_id` for live SSE progress via
  `subscribeToProgress`), `getCacheMoverRuns`. Types
  `CacheSettings` (now includes `mover_age_days`, `mover_excludes`,
  `mover_bwlimit`), `CachePoolInfo`, `CacheShare`, `CacheMoverRun`.

## i18n

All strings under `[cache]` in `src/i18n/{en,fr,de,es}.toml`. Key groups:
cache-tab chrome (`tab`, `title`, `subtitle`), configuration (`cache_pool_*`,
`mover_*`, `min_free_*`, `mover_age_label`, `mover_excludes_label`,
`mover_bwlimit_label`, `save_config`), mover (`run_mover`, `mover_started`,
`mover_hint`), shares (`add_share`, `share_name`, `mode`, `target_pool`,
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
- `updateCacheSettings({ cache_pool: null })` clears the designation; sending
  an empty/whitespace string does **not** (the handler only validates
  non-empty values, `src/handlers/cache.rs`). The frontend always sends
  `null` to clear.
- The mover validates that the cache pool and each share's `target_pool` both
  exist (`PoolService::scan_pools`) and that `target_pool != cache_pool`
  (`src/handlers/cache.rs`). It shells out to `rsync` and `df`, so it is
  CPU/IO-bound — run it off the request path (it already is: `run_mover` is the
  scheduler/endpoint target, not inline in a hot path).
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
- Backend `cargo check` passes. Live mover execution, cron scheduling, and the
  create/designate/remove-cache-disk flows in the Pools tab were not run
  against real hardware in this pass — the rsync/`df` logic and the
  pool-create/delete paths reuse the already-verified Storage machinery.

## Related

- [Storage](./storage.md), [Shares](./shares.md), [Snapshots](./snapshots.md)
- Backend: `src/cache.rs`, `src/cache_scheduler.rs`, `src/storage/cache_repo.rs`,
  `src/handlers/cache.rs`
