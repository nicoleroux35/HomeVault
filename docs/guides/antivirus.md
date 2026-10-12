# Antivirus (ClamAV on-demand + scheduled scanning)

**Purpose:** Scan files for malware using ClamAV's `clamscan` CLI, either
on demand (pick a path, scan now) or on a cron schedule. Deliberately
**not** real-time/on-access scanning — no `clamd` daemon is installed or
run — to stay within the low-power hardware budget documented in
the systemd unit `installer/install.sh` writes. Lives in the **Settings** page as the
Antivirus card.

## Backend

Migration `sqlx/migrations/018_antivirus.sql` adds three tables:

- `av_settings` — singleton row (`id = 1`): `quarantine_dir`,
  `scheduled_scan_enabled`, `scheduled_scan_cron` (default `0 3 * * 0`),
  `scheduled_scan_path` (nullable), `scan_archives`, `last_scan_at`.
- `av_scans` — one row per scan run: `id`, `scan_path`, `trigger`
  (`manual`/`scheduled`), `status` (`running`/`completed`/`failed`),
  `files_scanned`, `threats_found`, `started_at`, `finished_at`,
  `error_message`.
- `av_threats` — one row per detected threat: `id`, `scan_id` (FK),
  `file_path`, `signature_name`, `action` (`quarantined`/`left_in_place`),
  `quarantine_path`, `detected_at`, `resolved_at`.

Storage layer `src/storage/antivirus_repo.rs` (`AntivirusRepository`).
`AvSettings::default()`'s `quarantine_dir` is derived from
`crate::config::paths::quarantine_dir()` (which respects a `DATA_DIRECTORY`
override) — **not** a hardcoded path; a prior version hardcoded
`/var/lib/homevault/quarantine`, which was confirmed live to quarantine
files outside a customized data directory before being fixed.

Core logic `src/antivirus.rs`:

- `clamscan_available()` — version-check via a bare `Command::new
  ("clamscan")` (unprivileged; `clamscan` runs as the `homevault` user
  against paths it already owns, unlike `ufw`, so no `sudo` is needed or
  used here).
- `definitions_status()` — read-only mtime check of
  `/var/lib/clamav/{daily,main}.{cvd,cld}`. HomeVault never runs
  `freshclam` itself; that's the OS-level `clamav-freshclam` timer's job.
- `AV_SCAN_RUNNING: AtomicBool` + `AvRunGuard` — single-scan concurrency
  guard, same shape as `cache.rs`'s `MOVER_RUNNING`/`MoverRunGuard`.
- `run_scan` — spawns `clamscan -r -i --no-summary --move=<quarantine_dir>
  <path>` as a `tokio::process::Command` (not `spawn_blocking`: needs live
  stdout streaming for SSE progress and a killable handle for real
  cancellation), streams `FOUND` lines into `av_threats`, and records a
  `NotificationRepository` alert on completion. Exit codes: `0` clean,
  `1` threats found, `2` error (stderr is captured and included in the
  error message — a prior version discarded stderr and always blamed
  "permission issues" regardless of the real cause).
- `cancel_scan()` — signals the tracked PID via `kill -TERM` (not a
  `Child` parked in a mutex across an `.await`, which would be
  unsound/deadlock-prone); genuinely stops the process, unlike the
  generic `handle_cancel_job` in `progress.rs` which only drops the SSE
  job record.
- `resolve_threat()` — optionally deletes the quarantined file, marks
  `resolved_at`.

Scheduler `src/antivirus_scheduler.rs` (`run_scheduler_loop`): polls every
30s; when the next cron tick is due, `scheduled_scan_enabled` is set, a
`scheduled_scan_path` is configured, and no scan is already running, it
calls `run_scan`. Own file rather than folded into another scheduler so a
heavy scan tick can't block/be blocked by an unrelated domain's tick (e.g.
the cache mover). Spawned once at startup in `src/web/mod.rs`.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /antivirus/status` | auth | `clamscan_available`, `definitions`, `scan_running` |
| `GET /antivirus/settings` | auth | current config |
| `PUT /antivirus/settings` | admin | update config (scheduled path validated via `PathGuard`) |
| `POST /antivirus/scan` | admin | start a manual scan, returns `job_id` for SSE progress |
| `POST /antivirus/scan/cancel` | admin | kill the running scan |
| `GET /antivirus/scans` | auth | scan history (last 50) |
| `GET /antivirus/threats` | auth | all threats (resolved + unresolved) |
| `POST /antivirus/threats/{id}/resolve` | admin | mark resolved, optionally delete the file |

All handlers in `src/handlers/antivirus.rs`. Read endpoints use
`require_auth`; mutations use `require_admin`. Scan paths are validated
via `state.path_guard.validate(...)` — an admin can't be tricked (e.g. a
crafted request) into scanning arbitrary host paths outside the allowed
Explorer roots.

## Frontend

- `frontend/src/lib/components/settings/AntivirusCard.svelte` — settings
  form, scan trigger with live SSE progress bar (`/api/progress/{job_id}`,
  the same stream used by Explorer/Restic jobs), threat list with
  resolve/delete actions, scan history.
- Every path field (scan path, scheduled-scan path, quarantine dir) is
  **pick-only** — no manual typing. It uses the standard
  `theme/PathPickerField.svelte` (read-only display + a Browse button) which
  hosts `theme/FloatingPathPicker.svelte`: a **non-modal, draggable** picker
  panel (portalled to `<body>` to escape a desktop window's
  `transform`/`overflow`; no full-screen backdrop; closes on Escape / ✕ /
  "Select this folder"). It has a **left sidebar of locations** (physical
  disks, USB, RAID virtual disks, cloud, network — from
  `GET /api/explorer/disks`), each shown by its FS label if the user set one,
  otherwise the disk's make + model with the serial on the second line, plus
  its size; and a **list / grid** view toggle (remembered in `localStorage`
  `hv-pathpicker-view`). `FloatingPathPicker` takes the same
  `mode="folder"|"file"` contract as the older modal `theme/FolderPicker.svelte`
  and is the picker to reach for in new code; the older `theme/PathField.svelte`
  + `theme/FolderPicker.svelte` remain for their existing consumers.
- `frontend/src/lib/services/api.ts`: `getAntivirusStatus`,
  `getAntivirusSettings`, `updateAntivirusSettings`, `startAntivirusScan`,
  `cancelAntivirusScan`, `getAntivirusScanHistory`, `getAntivirusThreats`,
  `resolveAntivirusThreat`. Types `AvSettings`, `AvScan`, `AvThreat`.
- Wired into `frontend/src/routes/Settings.svelte` as `<AntivirusCard />`,
  after the "Privacy & Data" card.

## i18n

All strings under `[antivirus]` in `src/i18n/{en,fr,de,es}.toml`.

## Invariants

- On-demand + scheduled only — never real-time/on-access. Do not add a
  `clamd` integration without an explicit ask; it changes the resource
  profile this feature was deliberately scoped against.
- `clamscan` runs unprivileged as the `homevault` service user; scanning
  a path it can't read produces per-file "Permission denied" in
  `clamscan`'s own output, surfaced via the captured-stderr error path,
  not a crash.
- Virus definitions are never bundled or fetched by HomeVault itself —
  `installer/install.sh` installs `clamav` + `clamav-freshclam` (not
  `clamav-daemon`), and the OS-level `clamav-freshclam` timer keeps them
  current. `definitions_status()` is read-only.
- Only one scan (manual or scheduled) can run at a time, enforced by
  `AV_SCAN_RUNNING`; a second trigger is rejected immediately rather than
  racing two `clamscan` processes.

## Verified

Audited live end-to-end on 2026-07-16 — see the "Security suite" section
in `CLAUDE.md` for the full bug list and verification narrative (real
ClamAV installed, a real virus database downloaded via `freshclam`, and
the actual EICAR standard test file detected, quarantined, listed, and
resolved/deleted through the API).

## Related

- [Settings](./settings.md), [Anti-Ransomware](./ransomware.md),
  [Firewall](./firewall.md)
- Backend: `src/antivirus.rs`, `src/antivirus_scheduler.rs`,
  `src/storage/antivirus_repo.rs`, `src/handlers/antivirus.rs`
