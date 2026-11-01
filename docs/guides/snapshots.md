# Snapshots (pool rollback)

**Purpose:** Create, list, delete, and roll back ZFS/Btrfs pool snapshots;
schedule snapshots automatically with GFS (grandfather-father-son)
retention.

## Backend

Business logic in `src/snapshots.rs` (`SnapshotService`, consumed by the
restic backup flows (`src/restic.rs`) and the scheduled-snapshot engine).

**The backend is chosen per pool, never per host.** `SnapshotService::kind_for`
reads the pool's own technology from `PoolService::scan_pools()`: a ZFS pool
uses `zfs`, a Btrfs pool uses `btrfs`, and an mdadm array (or an
unclassified pool) is resolved by the filesystem mounted on it
(`/proc/mounts`); anything that is neither ZFS nor Btrfs returns
`SnapError::Unsupported`. This replaces the old "is the `zfs` binary
installed?" test, which sent every Btrfs pool down the ZFS branch on a host
with both toolkits installed (what `installer/install.sh` produces) and made
create/delete/rollback fail. `PoolBackend` resolves a pool **once** and
reuses it for every call of a run.

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
> | **Schedules (reads)** | `GET /api/snapshot-schedules/snapshots?pool=` | auth |
> | **Schedules (mutations)** | `GET/POST /api/snapshot-schedules`, `PUT/DELETE /api/snapshot-schedules/{id}`, `POST …/{id}/run` | **admin** |

## Scheduled snapshots + GFS retention (2026-10-13)

- `src/snapshot_scheduler.rs` — one loop creates a
  `{prefix}-YYYYMMDD-HHMMSS` snapshot per due schedule (default prefix
  `hv-auto`), then prunes expired generations. `src/storage/
  snapshot_schedule_repo.rs` + migration `046_snapshot_schedules.sql`.
- **One row per `(pool, prefix)`** — enforced by the unique index in
  migration `053_snapshot_schedule_unique_prefix.sql`. Two schedules sharing
  that pair list and prune the same snapshots (each deleting what the other
  keeps); the API answers **409** with an explicit message, and the Storage
  modal shows it inline.
- **A pool must exist** when a schedule is created or edited: a typo can
  otherwise only fail on every tick. The check is skipped when the host
  reports no pool at all (nothing to compare against), in which case the run
  itself reports the real error.
- Retention is GFS: keep the newest N **hourly**, **daily**, **weekly**,
  **monthly** and **yearly** generations; a snapshot belongs to exactly one
  tier (its youngest applicable window) and one generation bucket within it
  (hour / day / ISO week / month / year); each kept bucket keeps only its
  newest snapshot. A tier set to 0 keeps nothing in its window.
- **The tier windows follow the configured counts** (`keep × period`):
  hourly = `N` hours, daily = `N` days, weekly = `N` weeks, monthly = `N`
  × 30 days, yearly = everything older. They are clamped to be
  non-decreasing so a tier can never become unreachable. The old fixed
  24 h / 7 d / 30 d / 365 d windows silently ignored the operator's choice —
  "keep 48 hourly" could never hold more than 24 buckets, "keep 8 weekly"
  never more than 4.
- **Single-flight, per schedule and per pool.** A run claims both its
  schedule id and its pool in a process-wide registry (`RunGuard`, released
  on drop even if the run panics). A second run (a scheduler tick and a
  "Run now" at the same time, or two schedules on one pool) is refused with
  `RunScheduleError::Busy` → **409**, without touching the schedule row.
  Two concurrent prunes over the same pool would each delete what the other
  just decided to keep.
- **A failed prune is a failed run.** The snapshot may have been created,
  but if listing/deleting the old generations fails, retention did not run
  and the pool keeps filling up: the row is recorded as `failed` with the
  reason, and the admin is notified. Only the **transition** into failure
  notifies (event `snapshot.schedules`, see
  [Notifications](./notifications.md)) — a schedule failing every tick
  notifies once, not every 15 minutes.
- **Prune safety:** only snapshots whose bare name parses as
  `{prefix}-<timestamp>` are ever selected for deletion — manual snapshots,
  restic's pre-backup snapshots and other schedules' prefixes are
  untouchable. Selecting + destroying is `select_gfs_deletions` (pure,
  unit-tested) + `SnapshotBackend::delete`.
- Schedule CRUD/run-now is `handlers/snapshot_schedules.rs`, admin-only
  (reads of a pool's snapshot list are `require_auth`); cron validation
  reuses `compute_next_run`; pool/prefix names are validated (argv-safe)
  before reaching zfs/btrfs.
- Deleting a schedule keeps its snapshots (history is only removed by the
  prune policy, never as a side effect).

## Deadlines on every `zfs`/`btrfs` call (2026-10-24, ROADMAP X.2)

Every call of `SnapshotService` runs under a deadline now: reads (`zfs list`,
`zfs get`, `btrfs subvolume list`) at `SNAPSHOT_QUERY_TIMEOUT` 30 s, mutations
(`zfs snapshot|destroy|rollback -R`, `btrfs subvolume snapshot|delete`,
`sudo mkdir -p` for the Btrfs `.snapshots` directory) at
`SNAPSHOT_WRITE_TIMEOUT` 600 s. A wedged `zfs` used to hold the calling
`spawn_blocking` thread — and with it the scheduled-snapshot tick — forever;
it is now a `SnapError::CommandFailed` carrying the deadline message. The
timeouts are ceilings: a recursive rollback on a large dataset is the slowest
honest case. See [storage.md](./storage.md#deadlines-on-every-host-tool-2026-10-24-roadmap-x2).

## Frontend

- Snapshot UI lives in the Explorer (`Explorer.svelte`, read-only `.zfs`
  browsing), Restic (`Restic.svelte`) and Virtual Machines
  (`VirtualMachines.svelte`) routes. **Storage.svelte's Snapshots tab**
  owns the scheduled-snapshot manager: schedule table (pool, cron, prefix,
  retention counts, last status, next run), create/edit modal (GFS counts
  with tier hints), run-now, delete-with-confirm, and a snapshot list per
  pool tagging auto-managed snapshots. A refused or failed "Run now" is
  reported above the list (`snapshots-action-error`, translated message when
  the server sends none) — the button used to ignore the response, so a 409
  from a concurrent tick looked like a successful run.
- Translation keys: `restic.*snapshot*` / `restic.tab_snapshots` (backup
  snapshots), `explorer.no_snapshots` / `explorer.snapshot_readonly` /
  `explorer.snapshot_exit` (read-only `.zfs` browsing), `vm.snapshots_title`
  (saved states), `[storage.snapshots]` (schedules) — all 4 locales at key
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
- Schedule endpoints are admin-only (they destroy snapshots); reading a
  pool's snapshot list is `require_auth`. The prune filter can never match a
  snapshot whose name it didn't generate.
- The snapshot technology is a property of the **pool**, not of the host:
  two pools on the same machine may use different backends, and a host with
  both `zfs` and `btrfs` installed must not influence the choice.

## Immutable snapshots (2026-10-31, ROADMAP P2.5)

A **retention lock** keeps a snapshot from being destroyed before a date, even
by an admin who runs `zfs destroy` directly. The lock is deliberately **two**
things (`src/snapshot_locks.rs`):

1. a row in `snapshot_locks` (migration `068`), which is what HomeVault's own
   delete and prune paths consult before destroying anything;
2. a **filesystem-side hold** — `zfs hold homevault-lock <dataset@snap>` or a
   read-only Btrfs subvolume (`btrfs property set ro true`) — which is what
   makes `zfs destroy` itself fail.

A UI-only flag would be a promise the appliance could not keep, which is why
the second half exists.

### Lifecycle

| Step | What happens |
|------|--------------|
| **Place** | `POST /api/snapshots/locks {pool, snapshot, days, reason?}` — admin only. The hold is applied **first**; only then is the row written. If the row cannot be written the hold is removed again, so no invisible lock is left behind. |
| **While locked** | The scheduled-snapshot prune **skips** a locked snapshot and reports it (`kept N locked snapshot(s)`), so GFS retention never overrides an explicit lock. |
| **Expiry** | `locked_until` is the horizon. A lapsed lock is **not** silently treated as released: `snapshot_lock_scheduler` runs hourly, executes `zfs release` / `btrfs property … ro false`, and only then marks the row released — so until that succeeds the snapshot stays protected, failing closed. |
| **Release** | `DELETE /api/snapshots/locks/{id}`. After the horizon: a plain release. **Before** it: break-glass, refused unless the body carries `confirmation` equal to the phrase the API returned (`release <snapshot-name>`), and journaled as such. |
| **Audit** | Rows are never deleted, only marked released — who released it, when, and why (the break-glass reason is recorded) stays readable. Break-glass writes a `BREAK-GLASS:` line to the event journal. |

### Where it lives in the product

Storage → **Snapshots** tab → *Protection locks* (`Storage.svelte`). The list
shows each locked snapshot, its horizon (or that it has lapsed and is waiting
for the release loop), the reason and who placed it. Each snapshot in a pool's
snapshot list carries a **protect** action that opens a small dialog (days +
optional reason). Releasing before the horizon opens a dialog whose confirm
button stays disabled until the exact phrase is typed — the typed confirmation
reserved for irreversible actions. A release the server refuses (pool busy, the
hold owned by another process) keeps its reason in the section
(`snapshots-locks-error`) instead of closing as if it had worked; the list is
only reloaded once the release actually succeeded.

`GET /api/snapshots/locks?pool=<p>` returns the active locks, the last 50
released rows, `max_lock_days` (3650 — ten years; a longer horizon is a typo
and an accidentally permanent snapshot on a full pool is a real operational
problem) and the hold tag.

### Rules

- **One active lock per snapshot.** A second unreleased row would make "can
  this be deleted?" ambiguous; the uniqueness is a partial index in the
  migration, so history is still preserved.
- **The pool decides the mechanism**, never the host: the same resolution as
  every other snapshot operation (`SnapshotService::kind_for`).
- **Deletion is refused while a lock is active** — through HomeVault. A direct
  filesystem command is refused by the hold itself, which is the whole point.
- A snapshot that no longer exists is a stale lock: the release loop forgets
  the row rather than retrying a hold against nothing.

### Tests

- Unit (`src/snapshot_locks.rs`): the break-glass phrase, `decide_release`
  (normal / break-glass / refused), the argv vectors for both backends, quota
  and horizon validation, `protected_names`.
- Unit (`src/snapshot_lock_scheduler.rs`): `due_for_release` never selects an
  unexpired lock.
- Unit (`src/snapshot_scheduler.rs`): a locked snapshot survives a prune that
  would otherwise delete it, and is counted in the summary.
- Integration (`tests/snapshot_locks_integration.rs`): the endpoint contract
  (auth vs admin), duplicate-lock refusal, early release refused without the
  phrase and journaled with it, restart persistence.
- E2E (`tests/e2e/storage/snapshot-locks.spec.ts`): the list, the empty state,
  the lapsed wording, the typed-confirmation gate, and that a cancelled
  release fires no request. Mutations are mocked — no hold is ever placed.

## Related

- [Storage](./storage.md), [Backup / Restic](./backup-restic.md),
  [Scheduled tasks](./scheduled-tasks.md)
