# Snapshots (pool rollback)

**Purpose:** Create, list, delete, and roll back ZFS/Btrfs pool snapshots.

## Backend

Handlers in `src/handlers/snapshots.rs`, business logic in `src/snapshots.rs`
(`SnapshotService`, ZFS-first / Btrfs-fallback dispatch).

> **Fixed:** every handler here used to take no `State`/`HeaderMap` at all —
> all six endpoints, including destructive delete/rollback, were reachable
> with **no session token whatsoever**. Reads now use `require_auth`,
> mutations use `require_admin`.

| Endpoint | Method | Auth | Notes |
|----------|--------|------|-------|
| `/snapshots/availability` | GET | auth | whether ZFS/Btrfs tools are installed |
| `/snapshots/{pool}` | GET | auth | list snapshots for a pool |
| `/snapshots/{pool}` | POST | admin | create (`{ name, dataset?, recursive }`) |
| `/snapshots/{pool}/{name}/info` | GET | auth | ZFS-only detailed properties |
| `/snapshots/{pool}/{name}` | DELETE | admin | delete |
| `/snapshots/{pool}/{name}/rollback` | POST | admin | roll back to snapshot |

`{pool}` is the pool **name** as reported by `GET /storage/pools` — for ZFS
that's also the dataset identifier `zfs` commands expect, but Btrfs has no
such identifier, so every Btrfs-specific function in `SnapshotService`
resolves it to a real mount point first via
`PoolService::btrfs_mount_point` before shelling out.

## Frontend

- Snapshot UI lives within `frontend/src/routes/Storage.svelte`, as a
  per-pool expandable panel in the Pools tab (Zfs/Btrfs pools only).
- `frontend/src/lib/services/api.ts`: `getSnapshotsAvailability`,
  `getSnapshots`, `createSnapshot`, `deleteSnapshot`, `rollbackSnapshot`.
- Translation keys: `storage.snapshots_panel_title`, `storage.snapshot_create`,
  `storage.snapshot_rollback`, `storage.snapshot_delete_confirm`,
  `storage.snapshot_rollback_confirm`, `storage.snapshot_none` (all 4 locales).

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

## Related

- [Storage](./storage.md), [Backup / Restic](./backup-restic.md)
