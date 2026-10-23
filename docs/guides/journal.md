# System Journal

**Purpose:** Centralized log viewer — HomeVault's own rotating application
log, the systemd journal, and a set of `/var/log` text files — plus a
user-configurable auto-cleanup schedule for the application log.

## Sources

Three kinds, surfaced by `GET /journal/sources`:

- **`app`** — HomeVault's own log, written by the `tracing_appender`
  file-logging layer set up in `run()` (`src/web/mod.rs`). Always listed.
- **`system`** — the systemd journal, read via `sudo journalctl`. Always
  listed (every system this project supports has journald).
- **`/var/log/{syslog,auth.log,kern.log,daemon.log}`** — read via `sudo
  tail`. Each is only advertised if `Path::exists()` confirms it's actually
  present — `installer/install.sh` doesn't install/require `rsyslog`, and
  modern Debian/Ubuntu increasingly relies on journald alone, so these
  don't exist on every real deployment.

`journalctl` / `tail` are invoked via `sudo`. Since 2026-09-04 the process runs
as **root** and no sudoers file is generated (`configure_sudoers()` is dead code
— see [Architecture § Deployment model](../ARCHITECTURE.md#deployment-model-runs-as-root)),
so the bare `sudo` call succeeds; on an unprivileged dev box these sources
degrade to a clean `{success:false}`.

## Application-wide event log (the `app` source's content)

The `app` source isn't just whatever routine `tracing::info!` calls already
existed — it's a deliberate audit trail of business events across nearly
every subsystem: login/logout/failed-login, 2FA verify, password reset;
user/group create/delete/role/lock/2FA-disable/password-change; share
create/delete; Docker/VPN/backup/RAID/storage/network/settings/antivirus/
ransomware/firewall/Syncthing/rclone/Downloader/Rescue actions. Deliberately
scoped to business events, not a raw per-HTTP-request log (logging every
dashboard poll would bury what actually matters). Always plain English
regardless of the active UI language, matching the precedent for
log/check-style content elsewhere (Health, Downloader, Syncthing).

- `src/events.rs` — `log_event`/`log_event_warn`, the two functions every
  call site goes through. They write through the same `tracing` pipeline as
  everything else — no new storage, no new table.
- `src/handlers/journal.rs::parse_app_log_line`/`strip_target_prefix` strip
  the `homevault::mod::path: ` prefix `tracing_subscriber`'s default
  formatter prepends to every line, moving it into the entry's `unit` field
  (already used by the `system` source for systemd unit names) instead of
  leaving it as noise in the message text — without this, a clean sentence
  like `User 'admin' logged in successfully` would render as
  `homevault::handlers::auth: User 'admin' logged in successfully`.

## Backend

Handlers in `src/handlers/journal.rs`; scheduler in
`src/journal_cleanup_scheduler.rs` (`run_scheduler_loop`, spawned once at
startup); settings repo in `src/storage/journal_cleanup_repo.rs`
(migration `sqlx/migrations/021_journal_cleanup.sql`).

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/journal/sources` | auth | which sources exist right now |
| GET | `/journal/entries` | auth | `?source=&limit=&...` |
| DELETE | `/journal/clear` | admin | clears the `app` source's log files |
| GET/PUT | `/journal/cleanup-settings` | auth / admin | on/off + "every N days" |

The auto-cleanup schedule is deliberately just "on/off + every N days," not
a cron expression — `journal_cleanup_scheduler::run_due` does a plain
day-interval due-check, per explicit design choice.

## Frontend

- `frontend/src/routes/Journal.svelte`.
- `frontend/src/lib/services/api.ts`: `getJournalSources`,
  `getJournalEntries`, `clearJournalLogs`, `getJournalCleanupSettings`,
  `updateJournalCleanupSettings`.

## Invariants

- Clearing the `app` source removes the log files from the directory
  listing immediately, but the still-running process keeps writing into the
  now-unlinked file until the next day's rotation or a restart — no *new*
  activity appears in the `app` source in the meantime. This is standard
  behavior for deleting an actively-written log file, not a bug, and is
  called out in the clear-confirmation dialog's own copy.
- A new text-log source must be gated by `Path::exists()` (never
  unconditionally listed) and must have its reader binary (`tail`) covered
  by the sudoers grant, or it fails with a permission/not-found error the
  moment a user picks it. That grant is now per file (ROADMAP 7.4):
  `tail -n * /var/log/{syslog,auth.log,kern.log,daemon.log}` — one entry per
  name `text_log_path` can return, so adding a source means adding its
  grant. The journald read is pinned to the exact shape the viewer runs
  (`journalctl --no-pager --output=json --quiet … --output-fields=…`), which
  is why the `--output-fields` list is not optional: dropping or reordering
  a field breaks the sudoers match.
- The `[journal]` i18n table must never be declared twice across the 4
  locale files — TOML doesn't allow a duplicate table header, and it will
  crash the server at startup the same way a stray `;` inside a SQL
  migration comment once did (see `CLAUDE.md`'s Security suite section).
- **Editable since 2026-09-12** — `CLAUDE.md`'s frozen-area registry is gone, so
  `Journal.svelte`, `src/handlers/journal.rs`, `src/events.rs`,
  `src/journal_cleanup_scheduler.rs` and the `[journal]` i18n section are open
  to edits. Every `log_event`/`log_event_warn` call site elsewhere in the
  codebase is purely additive to whatever handler it's in; the invariants above
  stay binding.

## Related

- [Health](./health.md), [Security](./security.md), [API Contract](../API.md)
