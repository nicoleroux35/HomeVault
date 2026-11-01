# System Journal

**Purpose:** Centralized log viewer — HomeVault's own rotating application
log, the systemd journal, and a set of `/var/log` text files — plus a
user-configurable auto-cleanup schedule for the application log.

## Sources and who may read them

Three kinds, surfaced by `GET /journal/sources`:

- **`app`** — HomeVault's own log, written by the `tracing_appender`
  file-logging layer set up in `run()` (`src/web/mod.rs`). Always listed.
  **Session-level**: the process owns those files, so reading them grants
  the caller nothing it did not already have.
- **`system`** — the systemd journal, read via `sudo journalctl`.
- **`/var/log/{syslog,auth.log,kern.log,daemon.log}`** — read via `sudo
  tail`. Each is only advertised if `Path::exists()` confirms it's actually
  present — `installer/install.sh` doesn't install/require `rsyslog`, and
  modern Debian/Ubuntu increasingly relies on journald alone, so these
  don't exist on every real deployment.

`system` and the four `/var/log` files are **administrator-only**
(ROADMAP 20.1, 2026-10-23). `/var/log/auth.log` alone lists every account
name, source address and sudo attempt on the box, so the gate follows the
data, not the screen:

- `GET /journal/entries?source=auth` (or `system`/`syslog`/`kern`/`daemon`)
  answers **403** for a non-admin session, before any subprocess runs.
- `GET /journal/entries?source=app` stays readable for any valid session.
- `GET /journal/sources` only advertises what the caller may read — a
  non-admin sees `["app"]` and nothing else, so the screen never offers a
  source the backend will refuse.
- `DELETE /journal/clear` and `PUT /journal/cleanup-settings` are `admin`.
- The desktop hides the app entirely for a non-admin
  (`ADMIN_ONLY_APP_IDS` in `Desktop.svelte` filters the launcher, the
  global search and the dock), so a household user never lands on a screen
  whose host sources all answer 403.

`journalctl` / `tail` are invoked via `sudo`. Since 2026-09-04 the process runs
as **root** and no sudoers file is generated (`configure_sudoers()` is dead code
— see [Architecture § Deployment model](../ARCHITECTURE.md#deployment-model-runs-as-root)),
so the bare `sudo` call succeeds; on an unprivileged dev box these sources
degrade to a clean `{success:false}`.

## Reading semantics

One read is bounded on three axes, so a wedged host cannot hold a request
(ROADMAP 20, 2026-10-23):

| Bound | Value | Where |
|---|---|---|
| Per-command deadline | `JOURNAL_READ_TIMEOUT` = 20 s, child killed on expiry | `run_timed` |
| Concurrent host reads | 4 (`READ_PERMITS`, a `Semaphore`) | `handle_journal_entries` |
| Lines examined per read | `limit × 20`, hard-capped at 10 000 | `scan_limit` |
| Bytes read per application log file | 8 MiB, **tail** of the file | `read_file_tail` |

**The search filter runs before the page is cut.** The window is
`scan_limit(limit)` raw lines, not `limit`: a page of 10 scans the newest
200 lines, so a rare term older than the last 10 entries is still found
(ROADMAP 20.2). When the window fills up the response says so —
`meta.search_truncated: true` — and `Journal.svelte` renders
`journal.search_truncated` above the list rather than presenting a short
list with no explanation.

`run_timed` drains both pipes on their own threads before polling
`try_wait`: a 10 000-line `journalctl` dump overflows a pipe buffer, and a
child blocked on `write` never reaches the wait, so the deadline could not
fire at all.

Two inputs are validated before they reach an argv: an unknown `source` is a
`400`, and a `unit` filter must be a real systemd unit name (letters, digits,
`.`, `-`, `_`, `@`, `:`; at most 256 characters) — a typo now gets a 400 that
names the accepted characters instead of raw `journalctl` stderr in the UI.
The `unit` value is a single argv entry, so this was never an injection.

Reading an application log file that cannot be opened (permission denied) is
an **error**, not an empty log: `read_file_tail` returns the path and the io
error, and the screen shows it with a Retry. A failed load used to render as
the empty state, which is indistinguishable from "nothing matched".

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
  everything else — no new storage, no new table — **and** hand the sentence to
  the remote-syslog forwarder (see *Remote syslog forwarding* below), so a new
  audited mutation is forwarded without knowing that forwarding exists.
- `src/handlers/journal.rs::parse_app_log_line`/`strip_target_prefix` strip
  the `homevault::mod::path: ` prefix `tracing_subscriber`'s default
  formatter prepends to every line, moving it into the entry's `unit` field
  (already used by the `system` source for systemd unit names) instead of
  leaving it as noise in the message text — without this, a clean sentence
  like `User 'admin' logged in successfully` would render as
  `homevault::handlers::auth: User 'admin' logged in successfully`.

## Remote syslog forwarding (Log Center → Log Sending)

An admin can copy this box's audit stream to a collector over the network, so
the trail survives the appliance (a stolen or failed box keeps its history on
the collector) and a small office gets one place to read every machine.

`src/syslog_forward.rs`; endpoints in `src/handlers/syslog.rs`; settings in
`data/syslog_forward.json` (JSON storage, like DDNS and TLS — no migration).

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/settings/syslog` | **admin** | configuration + live queue counters (`pending`, `enqueued`, `dropped`, `sent`, `failed`) |
| PUT | `/settings/syslog` | **admin** | body `{ enabled, protocol, host, port, format, facility, ca_pem, include_app_events }`; validated, and `status`/`last_error`/`last_sent_at` are server-owned (a client cannot write them) |
| POST | `/settings/syslog/test` | **admin** | sends one real message now and answers with the collector's own reason on failure |

The card lives in **Settings → Monitoring** (`data-testid="syslog-*"`, save,
test, refresh, status pill, queue counters).

### The three rules that make it safe

1. **A log call never waits on the collector.** `events::log_event` only pushes
   a string into a 1024-entry queue (`enqueue`), then notifies the worker. Both
   switches are honoured: `enabled` **and** *Send HomeVault's own events* must be
   on, and the pair is cached for a second so the hot path is a flag read, not a
   JSON read. A
   slow, hung or dead collector can therefore never add latency to a sign-in or
   make a mutation fail. When the queue is full the **oldest** entry is dropped
   and counted — recent events are the ones worth keeping.
2. **One worker, bounded in every direction.** `run()` wakes on `enqueue`,
   takes at most 64 events (`BATCH_SIZE`), opens **one** connection for the
   batch and puts a deadline on each step (5 s to connect, 5 s to write). A
   batch that fails is dropped and counted, not retried forever: replaying an
   audit stream hours out of order is worse than a documented gap, and
   `last_error` plus the counters say exactly what was lost.
3. **The credential-less path is the whole path.** The settings carry no secret
   (a CA certificate is public material) and the forwarded text is the same
   plain-English sentence the Journal already shows, so forwarding cannot leak
   a token the audit line did not already contain.

### Framing

| Format | Shape |
|--------|-------|
| **RFC 5424** (default) | `<PRI>1 2026-10-31T12:00:00Z <host> homevault <pid> audit - <message>` |
| **RFC 3164** | `<PRI>Oct 31 12:00:00 <host> homevault[<pid>]: <message>` |

`PRI` is `facility × 8 + severity` (info = 6 for `log_event`, warning = 4 for
`log_event_warn`). `MSGID` is `audit` for the event stream and `test` for the
test button — the first thing an admin looks for on the collector. A missing
header field is rendered as `-` (RFC 5424's "unavailable"), never as an empty
string, which would shift every following field. A space inside a header field
becomes `_`, so a host name can never break the frame. A message longer than
the transport allows is truncated **on a character boundary** (a 1024-octet UDP
datagram, 8 KiB over TCP), so a multi-byte character is never cut in half.

RFC 3164's timestamp is space-padded to two digits (`Oct  3`, not `Oct 3`): a
strict collector's ABNF is `SP DIGIT DIGIT` and refuses the single-digit form.

### Transport and TLS

- **UDP** — one datagram per message; `connect()` only pins the destination, so
  an unresolvable name is reported once instead of per packet.
- **TCP** — one connection per batch, one message per write plus a trailing
  `
` (accepted by rsyslog, syslog-ng and journald's gateway), then flush.
- **TLS** — `tokio-rustls` with a client that trusts **only** the pinned CA:
  the system roots are deliberately not loaded, because a home collector
  presents a self-signed certificate for a local name and trusting every public
  CA would make the pinning decorative. The crypto provider is named explicitly
  (`ring`) since the dependency tree carries two (axum-server pulls
  `aws-lc-rs`), and `ClientConfig::builder()` refuses to guess.

### What is *not* forwarded

The host sources (`/var/log/syslog`, `auth.log`, …) are not re-shipped. The OS
already collects them, re-sending them would duplicate every line on the
collector, and their volume cannot be bounded by this queue. What leaves the box
is HomeVault's **own** audit stream — the same lines the `app` source shows.

## Backend

Handlers in `src/handlers/journal.rs`; scheduler in
`src/journal_cleanup_scheduler.rs` (`run_scheduler_loop`, spawned once at
startup); settings repo in `src/storage/journal_cleanup_repo.rs`
(migration `sqlx/migrations/021_journal_cleanup.sql`).

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/journal/sources` | auth | which sources this caller may read |
| GET | `/journal/entries` | auth / admin | `?source=` `app` = session, host sources = admin; `?unit=&priority=&limit=&search=`; answers `meta.scanned`/`meta.search_truncated` |
| DELETE | `/journal/clear` | admin | clears the `app` source's log files |
| GET/PUT | `/journal/cleanup-settings` | auth / admin | on/off + "every N days" |

The auto-cleanup schedule is deliberately just "on/off + every N days," not
a cron expression — `journal_cleanup_scheduler::run_due` does a plain
day-interval due-check (`plan_tick`), per explicit design choice.

**Only a successful cleanup advances `last_cleanup_at`.** Recording a failed
run as done postponed the retry by a whole `interval_days`, so a disk-full or
permission failure meant the logs were never pruned again for the configured
period (ROADMAP 20.3, 2026-10-23). A failure now returns `Err`, the loop logs
it, and the next hourly tick retries. The `None` (never-run) branch still
only establishes a baseline — cleaning there wiped the current day's audit
log on every restart.

## Frontend

- `frontend/src/routes/Journal.svelte`.
- `frontend/src/lib/services/api.ts`: `getJournalSources`,
  `getJournalEntries`, `clearJournalLogs`, `getJournalCleanupSettings`,
  `updateJournalCleanupSettings`.
- `ApiResponse` (`frontend/src/lib/types/api.ts`) carries the optional
  `meta` envelope this screen reads.

Failure feedback (simplicity rule 5): a failed load replaces the list with
`journal-error` + `journal-retry`; a failed Clear or Save keeps its modal open
and shows the message inside it (`journal-clear-error` /
`journal-cleanup-error`); a failed cleanup-settings read shows the page-level
`journal-action-error` banner. The Clear and Auto-cleanup buttons are
`$isAdmin`-gated, so a non-admin never sees a control that can only 403.

## Invariants

- Clearing the `app` source **truncates** the files the appender still holds
  open (`homevault.log`, `homevault.log.<UTC-date>`) and deletes only the
  already-rotated archives. It never unlinks the live inode: that made the UI
  show the file as gone while every event for the rest of the day was
  silently written to an orphaned inode. New activity therefore appears in
  the `app` source immediately after a clear.
- A partial clear is a failure. `clear_app_logs` collects every per-file
  error instead of returning on the first one, and `clear_outcome` reports
  them joined, so neither the admin button nor the scheduler can mistake
  "three of five files" for "done".
- A new text-log source must be gated by `Path::exists()` (never
  unconditionally listed) and must have its reader binary (`tail`) covered
  by the sudoers grant, or it fails with a permission/not-found error the
  moment a user picks it. That grant is now per file (ROADMAP 7.4):
  `tail -n * /var/log/{syslog,auth.log,kern.log,daemon.log}` — one entry per
  name `text_log_path` can return, so adding a source means adding its
  grant. The journald read is pinned to the exact shape the viewer runs
  (`journalctl --no-pager --output=json --quiet … --output-fields=…`), which
  is why the `--output-fields` list is not optional: dropping or reordering
  a field breaks the sudoers match. `-n<scan>` sits inside the grant's `*`
  wildcard, so widening the scan window needs no sudoers change.
- A new source id must be added to `source_kind` **and** to `text_log_path`:
  `source_kind` decides the privilege level, `text_log_path` the file. A
  source missing from `source_kind` is a 400 for everyone.
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

## Tests for remote syslog forwarding

- `src/syslog_forward.rs` — 23 unit tests: exact RFC 5424 / RFC 3164 bytes,
  `PRI` arithmetic, the `-` nil value, a space in a header field, day padding,
  the configured-format choice, UDP truncation on a character boundary, the
  strict-vs-lenient parse of protocol/format, the host/port/facility validation
  matrix, a half-filled *disabled* entry being saveable, the TLS CA requirement
  and its PEM parsing, `normalize`, drop-oldest queue bounds and counters,
  batch size and order, the status rules, and a scripted transport (batch
  rendering, empty batch, the collector's own refusal).
- `tests/syslog_forward_integration.rs` — 11: the admin-only matrix (no token,
  member, admin) on all three verbs, a save/reload round-trip, nine invalid
  payloads refused with nothing stored, a disabled entry still saveable,
  server-owned fields not writable, a **real localhost UDP collector** receiving
  the framed test message, a **real localhost TCP collector** receiving the
  newline-framed RFC 3164 line, an unreachable collector reported by the test
  button without failing the save, the test button refused while disabled,
  `events::log_event` queueing exactly one event only while forwarding is on,
  and `update_observation` recording a failed batch without reverting a
  destination the admin saved meanwhile (the worker never writes a stale
  snapshot).
- `tests/e2e/settings/syslog.spec.ts` — 16 `test(`: card shape and both
  dropdowns, the status pill's documented states (label, not colour alone),
  queue counters against the API, the CA field appearing only for TLS, a save
  round-trip, enable → `idle` → disable → `disabled`, a URL-as-host refusal with
  the reason on screen, TLS-without-CA refused, a fabricated `status` ignored,
  the test button success and refusal paths (mocked — no packet leaves the
  machine), the button disabled while off, a member refused on all three verbs,
  label/switch accessibility, duplicate ids, and no horizontal overflow with the
  CA textarea open. `afterAll` leaves forwarding **disabled**.

## Tests

- `src/handlers/journal.rs` — 20 unit tests (`priority_text`,
  `is_app_log_filename`, `text_log_path`, `strip_target_prefix`,
  `parse_app_log_line`, `seconds_to_iso`, and — added 2026-10-23 —
  `source_kind`, `scan_limit`, `validate_unit_filter`, `clear_outcome`,
  `run_timed` (fast command, failing exit code, killed at the deadline) and
  `read_file_tail` (whole file, seek-cut first line, unreadable file)).
- `src/journal_cleanup_scheduler.rs` — 4 unit tests on `plan_tick` (first
  test in the module): disabled, baseline instead of cleaning a first run,
  the interval boundary, and a zero interval treated as one day.
- `tests/journal_integration.rs` — 17: the auth split, host sources 403 for
  a non-admin / reachable for an admin, `/sources` filtered per role, the
  `app` source read for real from `DATA_DIRECTORY/logs`, search reaching an
  entry older than the page, `meta.search_truncated` when the window fills,
  an invalid unit filter as a 400, an unknown source as a 400, `clear`
  touching only `homevault.log*`, and the cleanup-settings default /
  round-trip / clamp.
- `tests/e2e/journal/journal.spec.ts` — 30 `test(`: page load, sources,
  filters (+ the bounded-search hint), the clear flow (never confirmed), the
  cleanup modal, navigation, responsive, accessibility (a failed read is an
  error with a retry, not the empty state), console/network, security
  (+ a non-admin session refused on host sources and not offered the tile),
  edge cases and the memory regression.

## Related

- [Health](./health.md), [Security](./security.md), [API Contract](../API.md)
