# Scheduled Tasks

**Purpose:** One generic cron engine for recurring admin-defined jobs —
pool snapshots, S.M.A.R.T. self-tests, host power actions, and (heavily
validated) single-executable commands — instead of one bespoke scheduler
per feature. Inspiration: OMV's `conf.system.cron.job` unified model.

## Backend

- `src/scheduled_tasks_scheduler.rs` — the loop (30 s poll, structural
  sibling of `restic_scheduler`) + the executor + `TaskAction`, the single
  validation gate for payloads. Cron parsing reuses the already-tested
  `restic_scheduler::compute_next_run` (5-field crontab).
- `src/storage/scheduled_task_repo.rs` — `scheduled_tasks` + run history
  (`scheduled_task_runs`, output tail capped at 8000 chars), migration
  `043_scheduled_tasks.sql`.
- `src/handlers/scheduled_tasks.rs` — HTTP CRUD + run-now + runs.
- Spawned in `src/web/mod.rs::run()` next to the other schedulers.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /scheduled-tasks` | admin | list with last-run summary + `next_run_at` |
| `POST /scheduled-tasks` | admin | create; payload validated **at creation** |
| `PUT /scheduled-tasks/{id}` | admin | update (also the enable/disable path); resets run summary |
| `DELETE /scheduled-tasks/{id}` | admin | delete (history goes too) |
| `POST /scheduled-tasks/{id}/run` | admin | run now, regardless of cron |
| `GET /scheduled-tasks/{id}/runs` | admin | run history, newest first (50) |

## Task types and payload shapes

| `task_type` | payload | effect |
|-------------|---------|--------|
| `command` | `{"command": "/abs/path", "args": [...]}` | runs the program (no shell), 30 min timeout |
| `snapshot` | `{"pool": "...", "recursive": false}` | creates `hv-auto-YYYYMMDD-HHMMSS` |
| `smart_test` | `{"device": "sda", "test_type": "short\|long\|conveyance\|offline"}` | `smartctl -t` |
| `power` | `{"action": "reboot\|shutdown\|suspend"}` | host power action (`src/power.rs`) |

## Invariants

- **Validation gate.** `TaskAction::from_parts` validates `task_type` +
  payload both in the create/update handler *and* again in the executor
  before running — a row written directly into the DB by another path still
  cannot make the executor do something the validators reject.
- **No shell.** `command` requires an absolute, existing, executable path;
  whitespace/metacharacters are rejected; args go through argv (no
  interpolation). Commands run as the server user with a 30-minute timeout.
- **Overlap guard.** A task whose previous execution is still running is
  skipped (in-memory running set; the loop is the only spawner).
- An unparseable cron is recorded on the task (`last_status='failed'`,
  clear error), not silently dropped.
- Every endpoint is `require_admin` — tasks execute host actions.
- Run output is truncated to 8000 chars so a chatty command cannot grow the
  DB unbounded.

## Frontend

- `frontend/src/routes/Tasks.svelte` (app id `tasks`, "Scheduled Tasks"):
  task table (name, type, cron, enabled, last status, next run), create /
  edit modal with a per-type payload form, runs drawer per task, "Run now"
  button, delete with confirm.
- `frontend/src/lib/services/api.ts`: `getScheduledTasks`,
  `createScheduledTask`, `updateScheduledTask`, `deleteScheduledTask`,
  `runScheduledTaskNow`, `getScheduledTaskRuns`.
- i18n keys: `tasks.*` (4 locales at key parity).

## Tests

- Unit: `TaskAction` validation matrix, `reference_time` 8-day cap,
  repo CRUD/run-history (`scheduled_task_repo.rs`), handler request
  validation (`handlers/scheduled_tasks.rs`).
- Integration: `tests/scheduled_tasks_integration.rs` (CRUD cycle, guards,
  run-now of `/bin/true`, validation rejections).
- E2E: `tests/e2e/tasks/tasks.spec.ts` (live server).

## Related

- [Power](./power.md), [Snapshots](./snapshots.md), [Storage](./storage.md),
  [Activity](./activity.md)
