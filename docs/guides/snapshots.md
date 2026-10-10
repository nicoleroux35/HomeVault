# Snapshots (pool rollback)

**Purpose:** Create, list, delete, and roll back ZFS/Btrfs pool snapshots.

## Backend

Business logic in `src/snapshots.rs` (`SnapshotService`, ZFS-first /
Btrfs-fallback dispatch), consumed by the restic backup flows
(`src/restic.rs`).

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

## Frontend

- Snapshot UI lives in the Explorer (`Explorer.svelte`, read-only `.zfs`
  browsing), Restic (`Restic.svelte`) and Virtual Machines
  (`VirtualMachines.svelte`) routes — there is no standalone Snapshots
  screen and no Pools tab in `Storage.svelte`.
- Translation keys: `restic.*snapshot*` / `restic.tab_snapshots` (backup
  snapshots), `explorer.no_snapshots` / `explorer.snapshot_readonly` /
  `explorer.snapshot_exit` (read-only `.zfs` browsing), `vm.snapshots_title`
  (saved states) — all 4 locales at key parity.

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
