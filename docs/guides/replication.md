# Snapshot replication

**Purpose:** keep a second copy of a dataset on another disk or another
machine. A job creates its own snapshot on the source, sends it incrementally
from the previous successful run, and prunes **only** the copies it made on the
destination.

## Where it lives

- **Screen:** desktop app **Replication** → `frontend/src/routes/Replication.svelte`
  (Backup → Replication).
- **Backend:** `src/replication.rs` (validation, naming, retention, pipeline
  construction), `src/replication_scheduler.rs` (the schedule loop and
  `run_job`), `src/handlers/replication.rs` (HTTP surface).
- **Storage:** `replication_jobs` (migration `069`). No credential is stored in
  the row.

## What one run does

1. **List the source snapshots** and pick the increment base — the snapshot the
   last successful run recorded (`last_snapshot`). An appliance that was off for
   a week sends **one increment**, not the whole dataset.
2. **Create this run's source snapshot**, named `hv-repl-<job id>-<UTC
   timestamp>`. The job never sends a snapshot an admin made; the prefix is what
   makes ownership unambiguous on both sides.
3. **Send and receive in one pipeline**, under a twelve-hour ceiling:

   - ZFS local: `sudo zfs send -s [-i base] pool@snap | sudo zfs receive -s dest`
   - ZFS over SSH: the same send piped into
     `ssh -p <port> -o BatchMode=yes -o ConnectTimeout=15 -o StrictHostKeyChecking=accept-new <user>@<host> zfs receive -s dest`
   - Btrfs local: `sudo btrfs send [-p <parent>] <snap path> | sudo btrfs receive <dest path>`

   `-s` on both ends is what makes an interrupted transfer **resumable**: the
   partially received stream is resumed instead of restarted.

   A bandwidth cap inserts `pv -L <kB/s>` into the pipe. When `pv` is not
   installed the job runs uncapped and **says so** (`bandwidth_note`) instead
   of pretending the limit applied.

4. **Prune the destination**, deleting only snapshots whose name carries this
   job's own `hv-repl-<id>-` prefix. A source snapshot is never deleted, and a
   snapshot another job or an admin made is never touched.
5. **Record the outcome** — `last_snapshot`, `last_status`, `last_error`,
   `last_run_at`, `next_run_at` — so the screen and the next run agree.

## Settings and their limits

| Field | Range | Notes |
|-------|-------|-------|
| Name | 1–64 chars, no control characters | Unique, case-insensitive |
| Backend | `zfs` / `btrfs` | Resolved per job, not per host |
| Destination | `local` / `ssh` | Btrfs over SSH is supported: the send runs locally and `ssh … sudo btrfs receive` runs remotely |
| Datasets | dataset paths, never `@snapshot` | The job creates its own snapshots |
| Interval | `0` (manual only) or 5–43200 minutes | 0 is "run when I say so" |
| Keep last | 1–365 | Destination-only retention |
| Bandwidth | 0 (unlimited) to 10 000 000 kB/s | `pv` must be installed to enforce it |

A local job whose destination dataset equals its source is refused.

## Dry run before anything moves

**Preview** (`GET /api/replication/{id}/preview`) returns the exact pipeline the
next run would execute — the snapshot name, the base it continues from, and the
shell-equivalent command line — without sending a byte. It is the fastest way to
check that an SSH user and dataset path are what you think they are.

## SSH authentication

A run authenticates with the SSH key the HomeVault account already has; no
password and no token is stored in the job row. Every stage is an **argv
vector**, never an interpolated shell string, and a log line carries the job id
and the snapshot names — never a credential or a raw argument list.

## API

| Method | Path | Auth |
|--------|------|------|
| `GET` | `/api/replication` | session (jobs + capability evidence) |
| `POST` | `/api/replication` | admin |
| `PUT` | `/api/replication/{id}` | admin |
| `DELETE` | `/api/replication/{id}` | admin |
| `GET` | `/api/replication/{id}/preview` | session (dry run) |
| `POST` | `/api/replication/{id}/run` | admin (run now) |

`GET /api/replication` also reports what this host can actually do — `zfs`,
`btrfs`, `ssh`, `pv` — and the screen prints it, so a missing tool is visible
before a job fails at 3 a.m.

## Interaction with retention locks

A replication destination is a normal dataset: locks placed there
([Snapshots](./snapshots.md)) are honoured like anywhere else, and a locked
destination copy is not pruned. A replication job is **not** a backup
destination tenant ([Backup destination](./backup-target.md)) — one copies
filesystem snapshots, the other receives `restic` repositories.

## Tests

- **Unit** (`src/replication.rs`): `validate_job` (every refusal above),
  `snapshot_name`, `choose_base`, `is_owned_snapshot`, and the argv vectors for
  all four pipeline shapes (local/SSH × ZFS/Btrfs, with and without `pv`).
- **Integration** (`tests/replication_integration.rs`): the endpoint contract
  (reads need a session, every mutation needs an admin), validation refusals,
  the preview shape, and the retention rules against a fake repository.
- **E2E** (`tests/e2e/backup/replication.spec.ts`): the list, the capability
  line, an incomplete job refused client-side, the fields a SSH job sends, the
  dry-run modal, run-now's confirmation, and the delete warning. All mutations
  are mocked — no `zfs`/`btrfs` command is ever executed.

## Btrfs over SSH and destination proof (P1-26, 2026-11-05)

- **Btrfs over SSH is supported.** The send runs locally and the receive runs
  as `ssh -p … -o BatchMode=yes … sudo btrfs receive <dataset>`; listing and
  deleting destination subvolumes use the same SSH prefix. The old
  `validate_job` refusal is gone, and `ssh_prefix` refuses an incomplete SSH
  target instead of building a half command.
- **Retries.** A transfer that fails is retried three times with exponential
  backoff (2 s, 4 s, 8 s). A dropped link is the common failure; a fresh
  attempt is safe because the destination snapshot is created once.
- **Destination proof.** After the last stage, the scheduler runs
  `destination_verify_command`: for ZFS it reads the destination dataset's
  `checksum` property (locally or over SSH), for Btrfs it runs
  `btrfs subvolume show` on the destination. A destination that cannot be
  read back fails the run — the copy is not recorded as a success. This proves
  the destination exists and is readable; it is **not** a byte-for-byte
  comparison of the snapshot (the send stream itself is the integrity
  guarantee, and ZFS/Btrfs checksums cover the blocks).

## Witness leases (P2-15, 2026-11-05)

`src/replication_witness.rs` implements a lightweight third vote between two
data sites:

- `may_promote` — a node may act as primary only while it holds an unexpired
  lease.
- `take_lease` — an unexpired lease held by another node is refused until it
  expires; the holder renews with a higher epoch.
- `split_brain` — two live leases from different holders are reported as a
  split brain.

The HTTP client (`WitnessClient`) speaks a small lease contract with an
external witness service:

| Method | Path | Body | Result |
|---|---|---|---|
| GET | `/lease` | — | current lease, or `404` |
| POST | `/lease` | `{ node, ttl_seconds }` | granted lease, or `409` when another node holds it |
| DELETE | `/lease` | `{ node }` | release |

Configuration is read at each run from `HOMEVAULT_WITNESS_URL`,
`HOMEVAULT_WITNESS_NODE`, optional `HOMEVAULT_WITNESS_TOKEN` and
`HOMEVAULT_WITNESS_TTL` (10–86 400 s, default 60). When a witness is
configured, **every replication run acquires/renews the lease first** and is
refused if another node holds it; without a witness the historical behaviour
is unchanged. `GET /api/replication/witness` reports the configuration and
current lease, `POST /api/replication/witness` acquires/renews it (admin).

**Reserve:** the witness service itself is external, and the promotion/
split-brain recovery runbook is not automated. The lease is not renewed during
a multi-hour transfer; its TTL only gates the *start* of a run.

## Related

- [Snapshots](./snapshots.md), [Backup destination](./backup-target.md),
  [Disaster recovery](./disaster-recovery.md), [Storage](./storage.md),
  [Scheduled tasks](./scheduled-tasks.md), [API Contract](../API.md)
