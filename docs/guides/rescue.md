# Rescue Tools

**Purpose:** Data-recovery and repair utilities: ddrescue, TestDisk, PhotoRec,
and mdadm diagnosis/repair. Every endpoint, including the read-only
`tools`/`devices` discovery, requires an `admin` session — real destructive
operations against real block devices, so there is no lower `auth` tier here.

## Backend

Router built by `src/handlers/rescue.rs::create_rescue_router`, nested at
`/api/rescue`. Service in `src/rescue/` (`RescueService`, `validator`,
`models`). Long jobs emit Progress SSE events.

| Endpoint | Method | Notes |
|----------|--------|-------|
| `/rescue/tools` | GET | tool availability (ddrescue, testdisk, photorec, mdadm) |
| `/rescue/devices` | GET | block devices |
| `/rescue/ddrescue/clone` | POST | `{ source, destination, options? }` |
| `/rescue/ddrescue/status/{log}` | GET | clone progress from logfile |
| `/rescue/testdisk/analyze` | POST | `{ device, deep_search? }` |
| `/rescue/testdisk/rebuild` | POST | `{ device, deep_search?, force? }` |
| `/rescue/photorec/recover` | POST | `{ device, output_dir, options? }` |
| `/rescue/mdadm/diagnose` | POST | `{ name }` |
| `/rescue/mdadm/repair` | POST | `{ name, action?, force?, verbose? }` |

## Frontend

- `frontend/src/routes/Rescue.svelte`.
- `frontend/src/lib/services/api.ts`: `getRescueTools`, `getRescueDevices`,
  `ddrescueClone`, `ddrescueStatus`, `testdiskAnalyze`, `testdiskRebuild`,
  `photorecRecover`, `mdadmDiagnose`, `mdadmRepair`.

## Invariants

- These tools operate on raw block devices — destructive. Confirm before running.
- Clone/recover jobs stream progress via `EventSource` to
  `/progress/{job_id}` (`api.subscribeToProgress`).
- ddrescue/photorec jobs are now registered in `PROGRESS_MANAGER`
  (`create_job_with_owner`, kinds `rescue_ddrescue` / `rescue_photorec`) instead
  of a bare UUID — this makes their SSE stream work (it never did before) and
  surfaces them in the desktop [activity taskbar](./activity.md).

## Related

- [Storage](./storage.md), [RAID](./raid.md), [API Contract](../API.md)
