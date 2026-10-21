# Rescue Tools

**Purpose:** Data-recovery and repair utilities: ddrescue, TestDisk, PhotoRec,
and mdadm diagnosis/repair. Every endpoint, including the read-only
`tools`/`devices` discovery, requires an `admin` session — real destructive
operations against real block devices, so there is no lower `auth` tier here.

## Backend

Router built by `src/handlers/rescue.rs::create_rescue_router`, nested at
`/api/rescue`. Service in `src/rescue/` (`RescueService`, `validator`,
`models`, `jobs`). Long jobs emit Progress SSE events.

| Endpoint | Method | Notes |
|----------|--------|-------|
| `/rescue/tools` | GET | tool availability (ddrescue, testdisk, photorec, mdadm) |
| `/rescue/devices` | GET | block devices |
| `/rescue/ddrescue/clone` | POST | `{ source, destination, options? }` |
| `/rescue/ddrescue/status/{log}` | GET | clone progress from a mapfile (confined path) |
| `/rescue/ddrescue/mapfile` | GET | directory holding the per-job default mapfiles |
| `/rescue/testdisk/analyze` | POST | `{ device }` |
| `/rescue/testdisk/rebuild` | POST | `{ device }` |
| `/rescue/photorec/recover` | POST | `{ device, output_dir, options? }` |
| `/rescue/mdadm/diagnose` | POST | `{ name }` |
| `/rescue/mdadm/repair` | POST | `{ name, action?, force?, verbose? }` |

### Options — only what the tool can actually honour

Every option below ends up in a tool's argv, so it is validated **before** the
tool starts and an invalid value is a `400` naming the field. Options that were
accepted but never reached the tool are gone rather than kept as switches that
change nothing:

| Tool | Accepted | Rejected with 400 |
|------|----------|-------------------|
| ddrescue | `logfile`, `block_size`, `retry_passes`, `direct_io`, `reverse`, `verbose` | `block_size` not a power of two in 512..=1 MiB, `retry_passes` > 100 000 |
| TestDisk | *(nothing)* | `deep_search` (scripted mode always deep-scans), `log_path` (TestDisk writes its own log), `force` (the table is never written back automatically) |
| PhotoRec | `partition_type` (Intel/EFI/Mac/Sun/None), `file_types` | `partition_type` outside that list; `file_types` entries that are not bare lower-case identifiers — they are spliced into PhotoRec's **comma-separated** `/cmd` grammar, so a comma or keyword would inject commands. `keep_broken` and `max_size` have no equivalent in that grammar and were removed |
| mdadm | `action` ∈ repair/check/rewrite/idle/frozen/resync/recover/reshape, `force`, `verbose` | any other `action` (it used to be forwarded verbatim to `--action=`) |

### Why the API can be handed raw device paths

All paths go through `src/rescue/validator.rs` (`SecurityValidator`):

- **Block devices** must be real block devices under `/dev`, are resolved
  through `canonicalize`, and are refused if they are the live root disk (found
  via `findmnt /`, not a hardcoded `sda`) or in `PROTECTED_DEVICES`.
  Device-name matching is partition-aware (`sda1` belongs to `sda`, `sdaa`
  does not).
- **Output directories** are confined to `/tmp`, `/var/tmp` and
  `paths::rescue_dir()`, judged component-wise (`/tmpfoo` is *not* inside
  `/tmp`). Symlinked parents are resolved before the containment check, and a
  final component that is a symlink, FIFO, socket or device node is refused —
  a planted `/tmp/x -> /etc/shadow` cannot be written through.
- **Mapfiles read back** by the status endpoint must already exist, be a
  regular file inside those same directories, be no symlink, and be ≤ 16 MiB.
  Before this check the endpoint took the raw URL segment and read it as the
  service user, i.e. an arbitrary-file read for any admin session.

### Clone safety

`check_clone_safety` (pure, unit-tested) runs before a clone starts:

1. source and destination must not resolve to the same device;
2. the destination must not be **in use** — no mounted filesystem or active
   swap on it or any child (`lsblk`), and no sysfs `holders` entry (i.e. it is
   not a member of an active md array, LVM volume group or dm-crypt mapping);
3. the destination must not be smaller than the source.

An unreadable size is a warning, never a silent pass or a hard failure.

## Cancelling a long job

A ddrescue clone or PhotoRec carve runs for hours and had no stop control: the
generic `DELETE /api/progress/{job_id}` dropped the job from the tracking map
and answered "Job cancelled" while the tool kept writing to the disk.

- `ProgressManager::register_cancel_hook` lets a job register the callback that
  actually stops it; `cancel_job` runs it and flips the job to
  `ProgressState::Cancelled`. The handler reports which of the three cases
  happened (`HookRan` / `NoHook` / `NotFound`, the last as a `400`).
- `src/rescue/jobs.rs` holds the registry of rescue jobs that own a live
  process. Both ddrescue and PhotoRec are spawned with `process_group(0)`, and
  cancelling sends `SIGTERM` to the whole group (`sudo kill -s TERM -- -<pgid>`),
  escalating to `SIGKILL` after 10 s. Signalling only the `sudo` wrapper would
  leave the real tool running.
- A cancellation that arrives between `register` and `spawn` is not lost: the
  handle's flag is set first and the kill is issued as soon as the pid exists.
- A cancelled clone keeps its mapfile (ddrescue writes one on `SIGTERM`), so it
  can be resumed; a cancelled carve keeps the files already recovered. Both are
  reported as *cancelled*, never as completed — a stopped clone has copied only
  part of the disk.

## Other invariants

- These tools operate on raw block devices — destructive. Confirm before running.
- Every subprocess runs under a hard timeout (`TOOL_PROBE_TIMEOUT` 10 s,
  `LSBLK_TIMEOUT` 15 s, `TESTDISK_ANALYZE_TIMEOUT` 120 s,
  `TESTDISK_REBUILD_TIMEOUT` 300 s, `MDADM_TIMEOUT` 60 s) via
  `RescueService::run_output`, which also sets `kill_on_drop`.
- The default ddrescue mapfile lives under `paths::rescue_dir()`, not `/tmp`:
  ddrescue writes it as root and `/tmp` is world-writable.
- PhotoRec's carve runs as root, so the recovered tree is `chown -R`'d to the
  admin who requested it (best effort; the account may not exist locally).
- Clone/recover jobs stream progress via `EventSource` to
  `/progress/{job_id}` (`api.subscribeToProgress`).
- ddrescue/photorec jobs are registered in `PROGRESS_MANAGER`
  (`create_job_with_owner`, kinds `rescue_ddrescue` / `rescue_photorec`) instead
  of a bare UUID — this makes their SSE stream work (it never did before) and
  surfaces them in the desktop [activity taskbar](./activity.md).

## Frontend

- `frontend/src/routes/Rescue.svelte`. TestDisk has no Simple/Advanced toggle
  (nothing left to put in an advanced block); ddrescue/PhotoRec/mdadm keep
  theirs. A running clone or carve shows a **Cancel** button that calls
  `api.cancelJob(jobId)`.
- `frontend/src/lib/services/api.ts`: `getRescueTools`, `getRescueDevices`,
  `ddrescueClone`, `ddrescueStatus`, `testdiskAnalyze`, `testdiskRebuild`,
  `photorecRecover`, `mdadmDiagnose`, `mdadmRepair`, `cancelJob`.

## Related

- [Storage](./storage.md), [RAID](./raid.md), [API Contract](../API.md)
