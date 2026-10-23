# Scheduled Tasks

**Purpose:** One generic cron engine for recurring admin-defined jobs —
pool snapshots, S.M.A.R.T. self-tests, host power actions, and (heavily
validated) single-executable commands — instead of one bespoke scheduler
per feature. Inspiration: OMV's `conf.system.cron.job` unified model.

## Backend

- `src/scheduled_tasks_scheduler.rs` — the loop (30 s poll, structural
  sibling of `restic_scheduler`) + the executor + `TaskAction`, the single
  validation gate for payloads. Cron parsing goes through
  `restic_scheduler::parse_schedule` (5-field crontab, day-of-week
  normalisation) so both schedulers share one parser;
  `scheduled_tasks_scheduler::next_run_at` evaluates it in a timezone.
- `src/storage/scheduled_task_repo.rs` — `scheduled_tasks` + run history
  (`scheduled_task_runs`, output tail capped at 8000 chars), migrations
  `043_scheduled_tasks.sql` and `054_scheduled_task_timezone.sql`.
- `src/handlers/scheduled_tasks.rs` — HTTP CRUD + run-now + runs.
- Spawned in `src/web/mod.rs::run()` next to the other schedulers.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /scheduled-tasks` | admin | list with last-run summary + `next_run_at` |
| `POST /scheduled-tasks` | admin | create; payload validated **at creation** |
| `GET /scheduled-tasks/{id}` | admin | one task — the status endpoint of a started run |
| `PUT /scheduled-tasks/{id}` | admin | update; resets the run summary (the type/payload changed) |
| `POST /scheduled-tasks/{id}/enabled` | admin | `{"enabled": bool}` — toggles only, keeps the run summary |
| `DELETE /scheduled-tasks/{id}` | admin | delete (history goes too) |
| `POST /scheduled-tasks/{id}/run` | admin | run now, regardless of cron; answers **`202`** + `{run_id, task_id, status}`; `409` while a run of that task is in progress |
| `GET /scheduled-tasks/{id}/runs` | admin | run history, newest first (50) |

## Task types and payload shapes

Request body (create/update): `name`, `task_type`, `cron`, `payload`,
`enabled`, and an optional `timezone` (IANA name, see below).

| `task_type` | payload | effect |
|-------------|---------|--------|
| `command` | `{"command": "/abs/path", "args": [...]}` | runs the program (no shell), 30 min timeout |
| `snapshot` | `{"pool": "...", "recursive": false}` | creates `hv-auto-YYYYMMDD-HHMMSS` |
| `smart_test` | `{"device": "sda", "test_type": "short\|long\|conveyance\|offline"}` | `smartctl -t` |
| `power` | `{"action": "reboot\|shutdown\|suspend"}` | host power action (`src/power.rs`) |

## Timezones (ROADMAP 12.2)

A cron expression is only meaningful together with the zone it is evaluated
in: `0 2 * * *` is 00:00 UTC in July and 01:00 UTC in January for
`Europe/Paris`, and a fixed offset gets that wrong twice a year. So:

- **`timezone` is per task** (`scheduled_tasks.timezone`, an IANA name such
  as `Europe/Paris`). `NULL`/blank means **follow the appliance timezone**
  — the General setting, which the Settings screen applies to the host with
  `timedatectl set-timezone` (`src/handlers/settings.rs`). A fresh install
  seeds that setting from the host's own zone at startup, so "server time"
  is never a lie.
- The value is validated twice: `validate_timezone` in the handler (refused
  with `400` naming the bad value, before anything is stored) and
  `parse_timezone` in the scheduler (a hand-edited row cannot make the
  engine silently fall back to UTC). Names are canonicalised on write
  (`europe/paris` → `Europe/Paris`).
- Evaluation converts the reference instant into the task's zone, lets the
  `cron` crate find the next occurrence there, and converts the result back
  to UTC — so DST transitions, skipped local hours and the weekday
  normalisation all behave. `chrono-tz` supplies the IANA database.
- `next_run_at` is computed in the task's zone, and the table shows the zone
  next to the cron (`tasks.timezone_server` when the task follows the
  appliance).

## Invariants

- **Validation gate.** `TaskAction::from_parts` validates `task_type` +
  payload both in the create/update handler *and* again in the executor
  before running — a row written directly into the DB by another path still
  cannot make the executor do something the validators reject.
- **No shell.** `command` requires an absolute, existing, executable path;
  whitespace/metacharacters are rejected; args go through argv (no
  interpolation). Commands run as the server user with a 30-minute timeout
  and `kill_on_drop(true)`, so a shutting-down runtime does not leave an
  orphan behind.
- **Overlap guard.** Runs are single-flight across the process: a
  process-wide `RUNNING` set claims a task under `TaskRunGuard` (released on
  drop, so a panic or an early return cannot wedge it forever) and **both**
  entry points acquire it — the scheduler tick and `POST /{id}/run`. A task
  whose previous execution is still running is skipped by the tick (logged,
  task row untouched) and refused by "Run now" with `409` and
  `Task '<name>' is already running`.
- **"Run now" does not run inside the request (ROADMAP 12.4).** The handler
  claims the task, opens the run row, moves the guard **and** the execution
  into a spawned task, and answers `202 Accepted` with the run id. Previously
  the run was awaited inside the handler with the guard on the request
  future: a client that went away dropped that future, the claim was released
  while the child kept running, and the next start was accepted — the same
  task running twice, with its row stuck on `running`. The outcome is read
  from the task row (`last_status` / `last_error`) or `GET /{id}/runs`; a
  failing command is a **run** failure, not a request error.
- **A run interrupted by a restart is closed.** `run_scheduler_loop` calls
  `close_interrupted_runs()` at startup: the guard is in-memory, so any row
  still `running` belongs to a process that is gone. It is marked `failed`
  with "Interrupted: the server stopped while this run was in progress".
- **An unrunnable task is paused, not retried (ROADMAP 12.3).** If the stored
  cron (or timezone) cannot be parsed, the tick records **one** failed run
  explaining why, then disables the task (`enabled = 0`, `next_run_at = NULL`,
  `last_status = 'failed'`, the reason in `last_error`). Retrying every 30 s
  used to fill the history table with identical failures for ever. The admin
  fixes the expression and re-enables the task — nothing is lost.
- **The enable switch only flips the switch.** `POST /{id}/enabled` does not
  re-submit the task, so pausing does not erase the last-run summary the
  screen is showing (the row used to send a full `PUT`, which resets it).
- Every endpoint is `require_admin` — tasks execute host actions.
- Run output is truncated to 8000 chars so a chatty command cannot grow the
  DB unbounded.

## Frontend

- `frontend/src/routes/Tasks.svelte` (app id `tasks`, "Scheduled Tasks"):
  task table (name, type, cron + zone, enabled, last status, next run),
  create / edit modal with a per-type payload form and a **Timezone** picker,
  runs drawer per task, "Run now" button, delete with confirm.
- The timezone picker offers the **whole IANA database** — `GET
  /settings/timezones`, the same list and the same `$lib/utils/timezones`
  grouping the Settings General card renders (regions as `<optgroup>`, `UTC`
  first), with "Server timezone" as the first entry = the empty value. It was
  a free-text field with a `Europe/Paris` placeholder: a typo only surfaced on
  save, and nothing told the admin which names were valid. A value the list
  does not hold (a legacy alias set through the API, or a list still loading)
  is rendered as its own option so the select can never silently display — and
  then save — a different zone.
- "Run now" shows a "started" notification and polls the list (3 s, at most
  10 attempts, paused while the tab is hidden) until the row leaves
  `running` — bounded, so a 30-minute command cannot turn the screen into a
  permanent poller.
- The arguments field splits on whitespace but honours double quotes:
  `--out "/mnt/my disk/file"` is two arguments, not three. The edit form
  re-quotes arguments containing a space so the value round-trips.
- `frontend/src/lib/services/api.ts`: `getScheduledTasks`,
  `createScheduledTask`, `updateScheduledTask`, `setScheduledTaskEnabled`,
  `deleteScheduledTask`, `runScheduledTaskNow`, `getScheduledTaskRuns`.
- i18n keys: `tasks.*` (4 locales at key parity).

## Tests

- Unit: `TaskAction` validation matrix, `reference_time` 8-day cap, the
  single-flight guard, `parse_timezone`/`next_run_at` (winter vs summer
  Paris, appliance-zone fallback, unknown zone is an error, weekday
  normalisation survives the timezone hop), repo CRUD/run-history/timezone
  round-trip/`set_enabled`/`pause_with_error`/`close_interrupted_runs`
  (`scheduled_task_repo.rs`), handler request validation
  (`handlers/scheduled_tasks.rs`).
- Integration: `tests/scheduled_tasks_integration.rs` (CRUD cycle, guards,
  `202` + run id then the row reaching `success`, a failing command recorded
  as a failed run, a concurrent second "Run now" refused with `409` and no
  extra run row, timezone validation/storage, enable toggle keeping the run
  summary).
- E2E: `tests/e2e/tasks/tasks.spec.ts` (live server) — including the
  timezone field, the asynchronous "Run now" and quoted arguments.

## Related

- [Power](./power.md), [Snapshots](./snapshots.md), [Storage](./storage.md),
  [Activity](./activity.md), [Settings](./settings.md)
