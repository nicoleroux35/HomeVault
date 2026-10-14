# Snapshots (pool rollback)

**Purpose:** Create, list, delete, and roll back ZFS/Btrfs pool snapshots;
schedule snapshots automatically with GFS (grandfather-father-son)
retention.

## Backend

Business logic in `src/snapshots.rs` (`SnapshotService`, ZFS-first /
Btrfs-fallback dispatch), consumed by the restic backup flows
(`src/restic.rs`) and the scheduled-snapshot engine.

> **Removed:** the standalone `/snapshots/{pool}` HTTP endpoints
> (`src/handlers/snapshots.rs`) are gone — every handler there took no
> `State`/`HeaderMap` at all, so all six endpoints, including destructive
> delete/rollback, were reachable with **no session token whatsoever**.
> Snapshot access is now scoped where it belongs:
>
> | Area | Endpoints | Auth |
> |------|-----------|------|
> | Explorer (ZFS, read-only) | `GET /api/explorer/snapshots?path=` — lists covering snapshots with a `browse_path` under `.zfs/snapshot/…` | auth |
> | Backups (restic) | `GET /restic/repos/{id}/snapshots`, `POST /restic/snapshots/delete` | auth / admin |
> | Virtual machines | `GET/POST /vm/{name}/snapshots`, `POST …/restore`, `DELETE …/{snap}` | admin |
> | **Schedules** | `GET/POST /api/snapshot-schedules`, `PUT/DELETE /api/snapshot-schedules/{id}`, `POST …/{id}/run`, `GET /api/snapshot-schedules/snapshots?pool=` | **admin** |

## Scheduled snapshots + GFS retention (2026-10-13)

- `src/snapshot_scheduler.rs` — one loop creates a
  `{prefix}-YYYYMMDD-HHMMSS` snapshot per due schedule (default prefix
  `hv-auto`), then prunes expired generations. `src/storage/
  snapshot_schedule_repo.rs` + migration `046_snapshot_schedules.sql`.
- Retention is GFS: keep the newest N **hourly** (24 h window), **daily**
  (7 d), **weekly** (30 d), **monthly** (365 d) and **yearly**
  (everything older) generations; a snapshot belongs to exactly one tier
  (its youngest applicable window) and one generation bucket within it
  (hour / day / ISO week / month / year); each kept bucket keeps only its
  newest snapshot. A tier set to 0 keeps nothing in its window.
- **Prune safety:** only snapshots whose bare name parses as
  `{prefix}-<timestamp>` are ever selected for deletion — manual snapshots,
  restic's pre-backup snapshots and other schedules' prefixes are
  untouchable. Selecting + destroying is `select_gfs_deletions` (pure,
  unit-tested) + `SnapshotService::delete_snapshot`.
- Schedule CRUD/run-now is `handlers/snapshot_schedules.rs`, admin-only;
  cron validation reuses `compute_next_run`; pool/prefix names are
  validated (argv-safe) before reaching zfs/btrfs.
- Deleting a schedule keeps its snapshots (history is only removed by the
  prune policy, never as a side effect).

## Frontend

- Snapshot UI lives in the Explorer (`Explorer.svelte`, read-only `.zfs`
  browsing), Restic (`Restic.svelte`) and Virtual Machines
  (`VirtualMachines.svelte`) routes. **Storage.svelte's Snapshots tab**
  owns the scheduled-snapshot manager: schedule table (pool, cron, prefix,
  retention counts, last status, next run), create/edit modal (GFS counts
  with tier hints), run-now, delete-with-confirm, and a snapshot list per
  pool tagging auto-managed snapshots.
- Translation keys: `restic.*snapshot*` / `restic.tab_snapshots` (backup
  snapshots), `explorer.no_snapshots` / `explorer.snapshot_readonly` /
  `explorer.snapshot_exit` (read-only `.zfs` browsing), `vm.snapshots_title`
  (saved states), `storage.snapshots_*` (schedules) — all 4 locales at key
  parity.

## Invariants

- Rollback is destructive and irreversible for ZFS (`zfs rollback -R`, works
  correctly in place) — the confirm dialog states this.
- **Btrfs rollback of the default mounted subvolume is not supported.**
  Every pool created by the Advanced Storage flow (see
  [Storage](./storage.md#advanced-pools)) mounts Btrfs at `subvolid=5`, the
  top level — there's no single-command way to revert that in place (unlike
  `zfs rollback`), and the naive `btrfs subvolume snapshot <snap> <mount>`
  an earlier version of this code attempted always fails (the destination
  already exists and is mounted). `rollback_btrfs_snapshot` now returns a
  clear error explaining to restore files manually from
  `<mount>/.snapshots/<name>` instead of silently no-op'ing or crashing with
  a confusing low-level error.
- Snapshot listing is scoped per pool.
- Schedule endpoints are admin-only (they destroy snapshots); the prune
  filter can never match a snapshot whose name it didn't generate.

## Related

- [Storage](./storage.md), [Backup / Restic](./backup-restic.md),
  [Scheduled tasks](./scheduled-tasks.md)
