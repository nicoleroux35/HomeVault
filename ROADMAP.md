# HomeVault Roadmap

Last updated: 2026-10-23 — current version: 1.0.0

## How to read this file

This file lists **what is left to do**, nothing else. Everything already fixed
was moved to the "Done" archive at the bottom (one line each, with a pointer to
the guide that carries the detail).

Every open item is written so it can be picked up cold:

- **Evidence** — `path:line` proving the defect exists in the current tree.
- **Findings** — numbered, each independently fixable.
- **Acceptance** — what must be true for the item to leave this file.
- **Tests** — what has to be added (the repo rule is that no feature ships
  without Rust unit + integration + Playwright coverage, see `CLAUDE.md`).
- **Docs** — which `docs/guides/*.md` must be updated in the same change.

An item leaves this file only when its acceptance criteria are met **and**
`TESTING.md` reflects the new counts.

### Verdict scale

| Verdict | Meaning |
|---|---|
| **P0 — Broken** | The feature does not do what it promises, or it can lose/corrupt user data. A home user hits this on the normal path. |
| **P1 — Needs work** | The feature works on the happy path but has holes a user, an attacker or a bad day will find. |
| **P2 — Small gap** | One concrete defect, or maintainability debt with a named trigger. |

> **Method note.** Every item below was found by reading the current code, not
> by re-reading `CLAUDE.md`. Where a guide claims an invariant, the claim was
> checked against the implementation — several "documented invariants" turned
> out to be false (they are listed as findings, and the guide is part of the
> fix). Line numbers are valid as of the commit that added this revision.

---

## Current state

The areas that have had a full "production-ready" pass are out of scope here:
**Docker** (2026-10-20), then **Firewall**, **Backup/restic**, **Users**,
**Groups**, **Rescue**, **Shares**, **Network**, **VPN** and the **app-proxy
auth under HTTPS** (all 2026-10-21), and **Health** (2026-10-22) — followed by
**Health's numbered follow-ups**, the **File Explorer security pass**, the
**Account / Security pass**, **disk quotas** and the **AI Assistant / RAG pass**
(all 2026-10-22), and on 2026-10-23 **Storage's sudoers scoping + residual call
sites**, **Apps Center / app-proxy**, the **RAID wizard's create path**,
**LUKS2 encryption**, the **System Journal's privilege + read semantics**,
**Downloader**, **Syncthing's REST client**, **Virtual Machines (Incus)** and
**Scheduled Tasks** — timezone-aware cron, pausing an unrunnable task, and
"Run now" off the request (see Done).

Everything else is still on its original implementation plus incremental
feature work. The pattern is consistent: **the screens render, the endpoints
answer 200, and the tests assert exactly that** — while the invariant the
screen exists to protect (a quota that limits, a health check that detects, a
retention that prunes, a cleanup that runs) is not enforced or not tested.

Two structural facts explain most of the list:

1. The test suites verify *shape* (status codes, `total`, DOM presence), not
   *behaviour*. None of the P0 defects closed on 2026-10-23 was caught by the
   50+ integration tests around the same code — each one needed a new assertion
   (revoke scope, redirect target, marker write failure) written with its fix.
2. Blocking work (`std::process::Command`, `std::fs`, `docker compose`) is
   called straight from `async fn` handlers in a dozen modules, while
   `mdns.rs`, `journal.rs`, `transfer_quota.rs` and `docker.rs` show the
   correct `spawn_blocking` shape. It is an inconsistency, not a gap in
   knowledge.

---

## Open work — index

| # | Area | Verdict | Blocking items |
|---|---|---|---|
| 10b | File Explorer — residual hardening | P2 | 3 |
| 11 | Metrics | P1 | 2 |
| 13 | Settings — remaining cards | P1 | 4 |
| 15 | Dashboard / Widgets | P1 | 3 |
| 16 | Cloud Sync (rclone) | P1 | 3 |
| 18 | Syncthing | P1 | 1 |
| 21 | Notifications | P1 | 3 |
| 22 | DB backup | P1 | 2 |
| 23 | Transfer quotas | P1 | 3 |
| 24 | Server control | P1 | 2 |
| 25 | Autostart | P1 | 2 |
| 26 | Wallpapers / Themes | P1 | 2 |
| 27 | Web shortcuts | P1 | 2 |
| 28 | Activity taskbar | P2 | 3 |
| 29 | mDNS | P2 | 2 |
| 30 | Quotas app | P2 | 1 |
| 31 | Docker — `api.ts` consolidation | P2 | 1 |
| 32 | Virtual Machines — residual | P2 | 2 |
| — | Cross-cutting (7 systemic) | — | 7 |

Total open findings: **48** (41 per-area + 7 cross-cutting).

**Suggested order.** No P0 remains — item 6 (Apps Center / app-proxy) was
closed on 2026-10-23, and **20 (System Journal) closed the same day**: the
host sources are now admin-only, so the remaining P1 list has no data leak
left. P1 now goes by blast radius. **9 (LUKS2) was closed on 2026-10-23**,
and **17 (Downloader) closed the same day**: `uri`/`dir`/`out` are now
validated against an allowlist and a confined root, the RPC port carries a
per-process secret, and the child is reaped and killed on shutdown. **18.2
closed on 2026-10-23**: a malformed API key is a named error instead of a
panicking handler, certificate verification is back on, and the key read left
the runtime. **19 (Virtual Machines) closed on
2026-10-23**: a `/wait` now gets the operation's own budget instead of the
45 s transport default, an expired wait reports "still running" with the
operation id instead of a false failure, and the blocking filesystem/socket
work left the async runtime. **12 (Scheduled Tasks) closed on 2026-10-23**,
together with 13.4's timezone half: a cron now runs in its task's own IANA
timezone (blank = the appliance zone, applied to the host with `timedatectl`),
an unparseable one pauses the task instead of retrying every 30 s, and "Run
now" answers `202` while the run continues outside the request. Next: **23
(transfer quotas)** — its check fails open, which is the last P1 that lets a
request through when the guard itself cannot answer.

---

## P0 — Broken

None. Item 6 (Apps Center / app-proxy) was closed on 2026-10-23 — see
"Done".

---

## P1 — Needs work

### 11. Metrics — failures are persisted as zeros

- [ ] **11.1 — A failed `/proc` read is stored as `0.0`, and the schema forbids
      gaps.** `src/metrics.rs:316-317,340` use `.unwrap_or(0.0)` for memory,
      load and CPU; migration `044_metrics_history.sql` declares every column
      `NOT NULL`, so a transient read failure writes a plausible-looking 0 %
      sample into the chart and gaps cannot be represented at all. This
      contradicts `docs/guides/metrics.md:32` ("gaps are visible as gaps, never
      interpolated").
      *Acceptance:* nullable columns + `None` on failure + the chart already
      draws null points.
- [ ] **11.2 — Rates divide by a constant 60 s.** `src/metrics.rs:320-323` uses
      `SAMPLE_INTERVAL_SECS` regardless of real elapsed time, and the loop uses
      `interval.tick()` with the default `MissedTickBehavior::Burst`
      (`:263-277`), so after a suspend or a stall it fires back-to-back samples
      that collide in the same minute bucket and are `INSERT OR REPLACE`d
      (`:335-337`) — under-reported throughput plus silently dropped data.

Also: three redundant `/proc` reads per tick (`:312,331`); `GET /metrics/latest`
returns HTTP 400 "No metrics collected yet" on a fresh install, which is a
normal first-boot state reported as a client error; `Metrics.svelte:67-71` does
not cancel an in-flight range request, so a slow `24h` response can overwrite a
newer `1h`; the empty-state heuristic only checks `cpu`/`mem` (`:174`), so a
temp-only dataset renders "no data".

*Tests:* `metrics.rs` 5, `handlers/metrics.rs` **0**, integration 5, E2E 4.
Nothing asserts null-on-failure or elapsed-time-based rates.
*Docs:* `docs/guides/metrics.md`.

---

### 13. Settings — the remaining cards

- [ ] **13.2 — `restart_nut()` cannot succeed.** `src/ups.rs:517-521` runs
      `systemctl restart nut-driver-enumerator.service nut-server`, but the
      sudoers grant (`src/setup.rs:877-888`) whitelists specific verbs/units and
      contains **no `nut-*` entry**. UPS config saves report success while the
      daemon never reloads. Related: `get_ups_status` / `get_ups_variables` /
      `get_ups_variable` (`src/ups.rs:116,135,151`) pass the request `name`
      straight into the `upsc` argv — `validate_ups_name` exists (`:327`) but is
      only used by add/delete config (`:343,505`).
- [ ] **13.3 — Email SMTP settings are silently ignored.** `smtp_server`, `port`,
      `username`, `password`, `use_tls` are persisted
      (`src/handlers/settings.rs:643-661`) but `EmailService` only passes
      `--from` to msmtp (`src/email.rs:236-244`); credentials must be
      pre-configured in msmtp's own file. The primary notification channel does
      not honour what the admin configured. Also `is_email_configured()` shells
      `which` three times per call (`:385-401`).
      *Acceptance:* either drive msmtp from the saved settings (or a real SMTP
      client) or remove the fields and say so.
- [ ] **13.4 — The UI language is not persisted.** `handle_set_language`
      mutates only the in-process locale (`handlers/settings.rs:376`) and
      `save_all_settings` has no language field, so the choice reverts to `en`
      on restart. *(The `timezone` half of this finding closed with ROADMAP 12
      on 2026-10-23: the setting is validated as an IANA name, applied with
      `timedatectl set-timezone`, seeded from the host on a fresh install, and
      is what the scheduled-task engine evaluates every task without a zone of
      its own in.)*
      *Acceptance:* the chosen language survives a restart and is applied
      before the first page renders.
- [ ] **13.5 — Dead and half-wired settings.** `scan_archives` (antivirus) is
      persisted (`handlers/antivirus.rs:81,109`;
      `storage/antivirus_repo.rs:34,117-131`) but `run_clamscan` never reads it
      and always passes `-r` without `--scan-archive`
      (`src/antivirus.rs:229-236`) — the toggle does nothing. SNMP `version` is
      write-only: `run_walk` hardcodes `2c` (`src/snmp.rs:189`) and
      `handle_snmp_interfaces` accepts `iface` and never uses it
      (`:154-159`). `settings_persistence::reset()` (`:257-270`) covers
      general/email/update/notification/appearance/AI/SNMP/TLS only — antivirus,
      ransomware, disk spindown, DB-backup settings and `ups.conf` all survive a
      "Reset to Defaults".

Also in this pass: `quarantine_dir` is unvalidated and interpolated into the
clamscan argv (`src/antivirus.rs:234`) with no `path_guard` check, unlike
`scheduled_scan_path` (`handlers/antivirus.rs:94-99`); `clamscan_available()` /
`definitions_status()` / `spindown_tool_available()` run `Command` inline in
async handlers (`antivirus.rs:78-117` called from
`handlers/antivirus.rs:50`, `antivirus_scheduler.rs:53`;
`handlers/power.rs:49-63`); `webhook_url` is unvalidated and the engine builds a
fresh `reqwest::Client::new()` with no timeout per dispatch
(`src/notification_engine.rs:362-365`); settings import re-serialises through
`serde_json::to_string` with no schema version and no per-field validation
(`handlers/settings.rs:1285-1327`).

*Tests:* every defect above lives in a module with **zero unit tests**
(`email.rs`, `notification_engine.rs`, `handlers/antivirus.rs`,
`antivirus_scheduler.rs`, `handlers/power.rs`, `autostart.rs`).
*Docs:* `docs/guides/antivirus.md`, `docs/guides/settings.md`,
`docs/guides/notifications.md`.

---

### 15. Dashboard / Widgets — global state, expired sessions, fabricated data

- [ ] **15.1 — Widget CRUD is global and unscoped.** `handle_add_widget` /
      `handle_remove_widget` / `handle_reorder_widgets`
      (`src/handlers/dashboard.rs:228-281`) only call `require_auth`: no
      ownership check, no admin check, and `username: None` is hardcoded at
      `:246`, so widgets are never attributed. Any authenticated household user
      can delete or reorder every widget on the appliance. `handle_add_widget`
      also accepts `id/wtype/title/size/row/col` with no validation.
      Related: `handle_widget_preferences` (`:2274-2283,2328-2337`) hand-rolls
      `SELECT username FROM sessions WHERE token = ?` with **no `expires_at`
      predicate**, so an expired token still reads and writes preferences.
      Both are also wrong on the wire: `json_error` is
      `StatusCode::BAD_REQUEST` (`src/utils/response.rs:87-93`), so a missing or
      invalid session comes back as **400**, not 401, and every client that
      branches on the status mis-reads an auth failure as a validation error.
      And `monitor_layout` stores the client's `layout_json` verbatim
      (`src/handlers/monitor_layout.rs:83-91`) — no parse, no schema, no size
      cap.
      *Acceptance:* widgets and layouts are per-user; every handler uses
      `require_auth`; the layout blob is validated and bounded.
- [ ] **15.2 — Untimed subprocess fan-out every 5 s.** `handle_server_details`
      is correctly offloaded (`:1439`), but `build_server_details` itself runs
      `nvidia-smi` (`:1798`), `rocm-smi` (`:1844`), `ip -json addr` (`:1994`),
      `lsblk` (`:2147`), **one `sudo smartctl -A` per disk** (`:2159`),
      `sensors` (`:2195`) and `dpkg-query` (`:1686`) with no timeout;
      `get_disk_health` adds three subprocesses per disk per call
      (`:815-918`); the browser re-fires every 5 s (`Dashboard.svelte:134`)
      with no `document.hidden` guard. A hung NFS mount or a spinning disk makes
      requests pile up.
      *Acceptance:* timeouts on every command, a shorter poll or SSE, and no
      polling while the tab is hidden.
- [ ] **15.3 — Fabricated values are presented as measurements.**
      `error_score = 10` hardcoded into the health score (`:983`);
      `has_server_update: false` hardcoded in both the cached and the live
      branch (`src/handlers/widgets.rs:320,347`);
      `"last test 2 last checked"` printed literally and a static `UP TO DATE`
      badge (`Dashboard.svelte:545,488`); `healthHint` always says "ok"
      regardless of score (`:168`); `GET /dashboard/widget/{id}` knows only
      `cpu`/`memory`/`uptime` and answers `memory` with the literal string
      `"/proc/meminfo"` instead of a value (`:186-223`).
      *Acceptance:* nothing shown as a measurement is a constant.
      Related: `PREV_STATS` is keyed on the literal `"cpu"` (`widgets.rs:117-123`)
      and `PREV_NETWORK_STATS` is shared too (`:194,235-240`), so two clients
      polling concurrently consume each other's delta and see 0 % or garbage;
      `total_idle - prev_idle` is an unchecked subtraction (`:127`).

Also: the dashboard emits `"critical"` (`src/handlers/dashboard.rs:530,683,1159,1226`)
while the documented vocabulary is `healthy|warning|unhealthy|unknown`
(`Health.svelte:29` declares exactly those four), and the value is then dropped:
`Dashboard.svelte:163` derives `healthStatus` and nothing reads it, because
`healthHint` is the constant `dashboard.health_hint_ok` (`:168`). CPU % is
`load1 / cores * 100`
(`:564`), which can exceed 100 %, while `src/metrics.rs:167` computes real
`/proc/stat` deltas — two contradicting numbers in one product; the Docker card
polls an admin-only endpoint every 5 s from every viewer
(`AppUsageBreakdown.svelte:174` vs `handlers/docker.rs:107`), so non-admins
issue a failing request every 5 s; `AppUsageBreakdown.svelte:159` uninstalls
with `force` behind a generic confirmation; `widgetData.ts:114-120` still routes
`app-update`/`server-update`, removed 2026-07-18; `monitorStore.ts:220-231`
overwrites local state on every login.

*Tests:* `monitor_layout.rs` **0** unit tests; nothing covers widget ownership
or session expiry. `TESTING.md` claims 37 Dashboard E2E tests, the spec has 41.
*Docs:* `docs/guides/dashboard.md`, `docs/guides/widgets.md`.

---

### 16. Cloud Sync (rclone) — no timeout, no shutdown cleanup, credentials at rest

- [ ] **16.1 — No timeout on any transfer, mount or unmount.**
      `src/handlers/rclone.rs:1119-1124,1389-1394,410,422,1434,1440,950,1002`:
      `sync`/`copy`/`fusermount -u`/`umount` await `.output()` forever. A stalled
      remote wedges the handler task and the job row stays `running`
      indefinitely.
- [ ] **16.2 — No shutdown cleanup.** The shutdown hook only cleans terminal
      PTYs (`src/web/mod.rs:2290`) — no rclone unmount, so stale FUSE mounts and
      orphaned processes survive a restart.
- [ ] **16.3 — Config written without restrictive permissions, and mounts are
      fire-and-forget.** The config is written with no `0600`
      (`:1510`) although remotes carry credentials; `mount` builds argv from an
      unvalidated `remote`/`mount_point` (`:950-971`) and calls `spawn()` without
      retaining the handle, so the app cannot report a failure or unmount it
      later; remote deletion is string surgery on the file rather than a parsed
      edit (`:1631-1648`).

Also: blocking `std::fs` on the async runtime (`:253-257,932`); the UI swallows
six failures (`CloudSync.svelte:234-346`) and derives job status from
`response.success` rather than the job state (`:322`); the mount point is
hardcoded to `/mnt/rclone/<name>` (`:260`); `local` remotes are filtered out of
the UI although the backend supports them (`:410`); **no scheduler exists** —
jobs only run when clicked.

*Tests:* 15 unit, 17 integration (which state explicitly at
`tests/rclone_integration.rs:31` that **no test performs a real transfer or
mount**, so 16.1–16.3 are untested by construction), 24 E2E.
*Docs:* `docs/guides/cloud-sync.md`.

---

### 18. Syncthing — it is a monitor, not a manager

- [ ] **18.1 — Folders and devices cannot be added or removed.** There is no
      route and no UI control for either (`Syncthing.svelte:175-215` is the
      entire action surface; `api.ts:1917-1943`). A user cannot onboard a new
      share or peer from HomeVault at all.
      *Acceptance:* add/remove folder and device, with validation and the same
      admin bar as the rest of the app.

18.2 (a malformed API key panicked the handler, and TLS verification was off)
was closed on 2026-10-23 — see Done. The blocking config discovery that sat
behind it (`find_syncthing_config` reading up to four files inline in every
handler) moved to `spawn_blocking` in the same pass, and the three empty
`else {}` action branches on the screen became one visible `Alert`.

Remaining "also" item, untouched: an N+1 `db/status?folder=` round-trip per
folder on every poll (`src/handlers/syncthing.rs:585-613`, deliberate —
Syncthing has no bulk call). The HTTP client has a real 10 s timeout
(`HTTP_TIMEOUT`, `:44`) — the only one of the four network apps with a bound.

*Tests:* 18 unit, 8 integration, 23 E2E. *Docs:* `docs/guides/syncthing.md`.

---

### 21. Notifications — blocking email in the loop, no per-user state

- [ ] **21.1 — Blocking email send and an untimed webhook inside the async
      engine.** `src/notification_engine.rs:345` calls
      `EmailService::send_system_alert`, which shells out with
      `std::process::Command` + `wait_with_timeout` (`src/email.rs:181-253`), on
      the runtime with no `spawn_blocking`; `:362-365` builds a fresh
      `reqwest::Client::new()` per dispatch with no timeout. A slow MTA or a
      black-holed webhook stalls the notification loop.
- [ ] **21.2 — There is no per-user read state, although the code says there is.**
      `src/storage/notification_repo.rs:10,19` claim it, but `read_at` is a
      single global column (`sqlx/migrations/015_notifications.sql:13`) and
      `mark_read`/`mark_all_read` (`:112,122`) have no user predicate — one
      admin clicking "mark all read" clears the badge for every account.
      `mark_read`/`delete` are `require_auth` with no owner
      (`src/handlers/notifications.rs:72,86,105`), so any user can delete the
      alert trail.
      *Acceptance:* either per-user state (a join table) or a documented,
      correctly labelled global feed — not the current half-and-half.
- [ ] **21.3 — No pagination, and "clear all" is N unawaited DELETEs.**
      `list_recent(50)` hard-caps (`handlers/notifications.rs:49`,
      `notification_repo.rs:94`); older alerts are unreachable and the clear loop
      only clears the visible 50. `NotificationCenter.svelte:111-113` fires one
      request per id, unawaited and unchecked, and `:96,102,107` fire mark-read
      and delete with no error handling while the store updates optimistically
      (`stores/notifications.ts:163`) — a failure looks like a success until the
      next poll.

*Tests:* `notification_engine.rs` **0**, `handlers/notifications.rs` **0**,
integration 18, no dedicated E2E.
*Docs:* `docs/guides/notifications.md` (`:125` already documents the
in-memory dedup reset as accepted debt — keep, but fix 21.1–21.3).

---

### 22. DB backup — the documented serialisation does not hold

- [ ] **22.1 — A new `DbBackupService` per request means two concurrent
      `VACUUM INTO`s.** `src/handlers/db_backup.rs:49,62,91` construct a fresh
      service, and `running` is a per-instance `Arc<Mutex<()>>`
      (`src/db_backup.rs:113,121`); the scheduler spawns its own
      `DbBackupService::new(db.clone())` (`src/web/mod.rs:1971`). A manual click
      during a scheduled snapshot therefore runs both — exactly what
      `docs/guides/backup-restic.md:43` promises cannot happen.
      *Acceptance:* one process-wide lock (or one service in `AppState`).
- [ ] **22.2 — `last_error` is hardcoded `None`, and prune can eat the
      pre-migration restore point.** `src/db_backup.rs:292` reports "no error"
      for every failure (unwritable dir, disk full), so the status card can
      never show a failed snapshot. `prune` keeps the newest `retain` by name
      with no marker filter (`:261-283`), so a forced
      `homevault-<ts>.pre-migration.db` (`:361`) can be pruned like any other.

Also: blocking `std::fs` in async fns (`:178,222,229,236,241,270`) and sync
`status()`/`list()` called straight from the async handler
(`handlers/db_backup.rs:49-50`).

*Tests:* 5 unit, 12 integration, no E2E. *Docs:* `docs/guides/backup-restic.md`.

---

### 23. Transfer quotas — fail-open, lifetime counters, and a client-side admin gate

- [ ] **23.1 — A DB error disables every cap silently.**
      `src/handlers/explorer.rs:1041-1047,1143,1203-1212,1639-1643` treat a
      quota-load error as "no quota" (`unwrap_or_default`). A transient DB
      failure lets every transfer through.
      *Acceptance:* fail **closed** with a clear error, or fail open with a
      loud log and a UI warning — but decided, not accidental.
- [ ] **23.2 — Counters are lifetime, not periodic.**
      `sqlx/migrations/022_transfer_quotas.sql:15,18` are plain accumulators;
      `record_usage` only ever adds (`src/transfer_quota.rs:195-207`) and there
      is no period column and no scheduler job to reset them. "100 GB/month" is
      really "100 GB ever": once hit, the user is blocked until an admin edits
      the row by hand.
      *Acceptance:* a period (day/week/month) with a reset job, or rename the
      feature to what it does.
- [ ] **23.3 — Any user can read any subject's usage.**
      `src/handlers/transfer_quotas.rs:54,84` are `require_auth`, so
      `GET /transfer-quotas/user/<someone-else>` returns their limits **and
      consumed bytes**. The admin gate exists only in `Quotas.svelte:104-108`,
      which `docs/guides/transfer-quotas.md:100-103` treats as the enforcement
      boundary. It is not one.

Also: group membership is resolved by shelling `id -nG` per request
(`src/transfer_quota.rs:136`, correctly inside `spawn_blocking` but still a fork
per transfer); `QUOTA_LOCKS` never removes entries (`:272-291`); `bytes as i64`
(`:207`) is unchecked; **enforcement covers File Explorer only** — SMB/NFS/FTP/
WebDAV/SFTP are not metered or capped regardless of configuration
(`docs/guides/transfer-quotas.md:14-20`, accurate).

*Tests:* `transfer_quota.rs` 9, `handlers/transfer_quotas.rs` **0**,
`tests/quotas_integration.rs` was **deleted** 2026-09-09, E2E 36 `test(` / 38
runtime (on the read-only app, not on the engine). *Docs:*
`docs/guides/transfer-quotas.md`.

---

### 24. Server control — the user cannot tell success from refusal

- [ ] **24.1 — Empty success and error branches in the power flow.**
      `frontend/src/routes/Desktop.svelte:1224-1234`:
      `if (response.success) { } else { }` for **both** restart and shutdown.
      On failure the modal closes and nothing happens; a rejected command is
      indistinguishable from a slow one. `docs/guides/server-control.md:25-26`
      already specifies the intended behaviour ("redirect to login or show a
      'server unavailable' state") — the code does not implement it.
- [ ] **24.2 — Two divergent privilege paths for the same button.**
      `src/handlers/system.rs:68,128` run `shutdown -r now` / `shutdown -h now`
      via `tokio::process::Command` with **no `sudo`**, while
      `src/power.rs:151-165` runs `sudo systemctl reboot`. On a box where the
      service user cannot call `shutdown`, one endpoint fails and the other
      works. `/api/server/restart` and `/api/power/reboot` are duplicates; the UI
      uses one, so the other is an untested second path.

*Tests:* `handlers/system.rs` **0** unit, `power_integration.rs` 4,
`settings/power.spec.ts` 4. *Docs:* `docs/guides/server-control.md`,
`docs/guides/power.md`.

---

### 25. Autostart — a blocking `systemctl` in an async handler, with the error thrown away

- [ ] **25.1 — `is_enabled()` / `set_enabled()` run on the runtime.**
      `src/handlers/settings.rs:566,595` call `std::process::Command`
      (`src/autostart.rs:34-56`) with no `spawn_blocking`; `sudo systemctl
      enable` can block for seconds on sudo PAM or a systemd job. Contrast
      `mdns.rs:449` and `transfer_quota.rs:136`, which wrap correctly.
- [ ] **25.2 — The real error is discarded.** `handlers/settings.rs:595+` maps
      any `Err` to `settings.autostart_update_failed`, throwing away the stderr
      that `set_enabled` carefully returns (`autostart.rs:50-52`), so
      "polkit refused" and "unit not found" are indistinguishable.

*Tests:* `src/autostart.rs` has **0 tests** and the sudo/`systemctl` argv shape
is untested; the only E2E is the staged-then-saved behaviour
(`tests/e2e/settings/settings.spec.ts:518`).
*Docs:* `docs/guides/settings.md`.

---

### 26. Wallpapers / Themes — a non-admin never receives the server appearance

- [ ] **26.1 — Hydration calls an admin-only endpoint.** `App.svelte:39` calls
      `api.getAllSettings()`, whose handler is admin-only
      (`src/handlers/settings.rs:1189-1191`). For a non-admin the call fails, the
      `catch` swallows it (`App.svelte:54-56`), and the desktop silently keeps
      that browser's `localStorage` values — two users on one appliance see
      different wallpapers with no explanation. `PUT /settings/appearance` is
      also admin-only (`:1106`), so a non-admin cannot change their own accent.
      *Acceptance:* a non-admin-readable appearance endpoint (or per-user
      appearance), consumed by the boot hydration.
- [ ] **26.2 — `theme` and `density` are persisted no-ops.** Written and
      round-tripped (`handlers/settings.rs:1111,1120,1163,1166`) but nothing
      consumes them — `grep -r "data-theme\|data-density" frontend/src` returns
      **zero** hits. A user can save a "theme" that changes nothing.
      *Acceptance:* wire them, or delete the fields and the round-trip.

Also: the wallpaper is stored as a JSON string inside the settings blob
(`:1115`) and both readers parse with a `try/catch` fallback
(`Settings.svelte:392-405`, `App.svelte:45-52`), so one malformed value silently
resets the user's wallpaper instead of reporting corruption.
*Docs:* `docs/guides/wallpapers.md`, `docs/guides/settings.md`.

---

### 27. Web shortcuts — the 512 KB cap does not bound anything

- [ ] **27.1 — The body is fully buffered before the cap applies.**
      `src/handlers/shortcuts.rs:376-387` does `resp.bytes().await` and only then
      truncates to `MAX_BODY_BYTES`; a multi-GB response is fully materialised
      first. The 10 s timeout (`:58,319`) is the only real bound.
      *Acceptance:* a streaming reader that stops at the cap.
- [ ] **27.2 — One redirect hop, and a DNS-rebinding window.**
      `fetch_following_one_redirect` handles exactly one hop (`:355-370`), so a
      second redirect returns the redirect body as "the page" and metadata
      silently comes back empty on common CDN/short-link chains.
      `resolve_public` (`:134-157`) validates the resolved IPs but reqwest
      re-resolves at connect time, so a hostile DNS answer that changes between
      check and connect reaches an internal address.
      *Acceptance:* follow the chain with per-hop validation, and pin the
      connection to the validated IP.

Also: a new `reqwest::Client` per request (`:316-325`), losing connection-pool
reuse.
*Tests:* 7 unit, 3 integration, 8 E2E. *Docs:* `docs/guides/shortcuts.md`.

---

## P2 — Small gaps

### 10b. File Explorer — residual hardening (non-blocking leftovers of the closed item 10)

- [ ] **10.5 — `validate_path_access` returns the raw path for admins and
      `PathGuard::validate` accepts a non-existent final component**
      (`src/handlers/explorer.rs`, `src/config/explorer_config.rs:259`), so the
      last component of an admin path is never canonicalised — a symlink
      planted after validation escapes the root (TOCTOU).
      *Acceptance:* the final component is resolved or re-validated after the
      fact for admin paths.
- [ ] **10.6 — Upload chunks are fully buffered.** `/upload/chunk` takes the
      whole chunk as an in-memory `Bytes` body and the global body limit is
      10 GiB (`src/web/mod.rs:1764`), so a single huge chunk is still a
      RAM spike. *Acceptance:* streamed body writes capped by the session's
      `total_size`.
- [ ] **10.7 — `SHARE_SEMAPHORE.acquire_owned().await.unwrap()` can panic**
      (`src/handlers/share_links.rs:463`) if the semaphore is closed.
      *Acceptance:* a mapped error, not an unwrap.

---

### 28. Activity taskbar

- [ ] **28.1 — No backoff on poll failure.** `activityStore.ts:44,83` polls
      every 2000 ms unconditionally, so a failing backend is hit 30×/min per open
      desktop. Add exponential backoff.
- [ ] **28.2 — Failed jobs vanish instead of showing a failure row.**
      `src/handlers/activity.rs:82` surfaces only `Pending | Running`, so a job
      that failed one second ago disappears (it only reaches the bell).
- [ ] **28.3 — Phase 2, documented.** Syncthing folder sync and Storage
      operations (mdadm resync, `zpool`/`btrfs` scrub, `luksFormat`, RAID
      rebuild) need a poller turning live system state into synthetic jobs, plus
      a live-hardware pass (`docs/guides/activity.md:99-108`). A
      `/api/activity/stream` could replace the poll.

### 29. mDNS

- [ ] **29.1 — `unpublish()` is dead code.** `src/mdns.rs:431` has no caller, so
      disabling mDNS leaves the Avahi service file and the host-name override on
      disk. Wire it into the settings path.
- [ ] **29.2 — `systemctl restart avahi-daemon` on any hostname change**
      (`:411`) briefly drops every other Avahi advertisement on the host
      (printers, shares). A reload would do.

### 30. Quotas app

- [ ] **30.1 — The admin gate is client-side only.** `Quotas.svelte:104-108`
      redirects non-admins, but `GET /transfer-quotas/{type}` is `require_auth`
      — same root cause as 23.3; fixing 23.3 fixes this.

### 31. Docker — `api.ts` consolidation (maintainability)

- [ ] **31.1 — `Docker.svelte` drives the whole surface through its own
      `authedFetch`** while `api.ts` exposes 6 Docker methods used by the desktop
      monitor. The two are behaviourally identical today (same session + CSRF
      headers, checked 2026-10-12), so this is debt, not a bug. ~50 new wrapper
      methods and ~100 rewritten call sites, with `docker.spec.ts` (58 tests) as
      the only net. If taken up: one tab at a time, E2E green between each.

### 32. Virtual Machines — residual (leftovers of the closed item 19)

- [ ] **32.1 — The screen relay is a raw TCP pipe with no framing or
      heartbeat.** `screen_relay` (`src/handlers/vm.rs:1380-1482`) pipes
      whatever arrives on the WebSocket to `127.0.0.1:5900+N` and back, so a
      half-open connection (laptop lid closed, Wi-Fi dropped) leaves the task
      and the qemu VNC session alive until TCP gives up — there is no ping and
      no idle timeout. *Acceptance:* a heartbeat plus an idle deadline that
      closes both sides, or a framed protocol.
- [ ] **32.2 — An ISO download runs to completion before the 10 MB floor is
      checked.** `run_iso_download` (`:1650-1742`) streams the whole body to
      disk and only then calls `finalise_iso`, so a link that returns a
      multi-GB wrong file is fully written before it is rejected. The floor now
      runs on the blocking pool (2026-10-23) but not *before* the bytes land.
      *Acceptance:* stop the stream as soon as the declared length is below the
      floor, and cap the body at a sane ISO size.

---

## Cross-cutting — fix once, unblocks many

- [ ] **X.1 — Blocking work on the async runtime.** `std::process::Command`,
      `std::fs` and `docker compose` are called straight from `async fn` in:
      Health (`web/mod.rs:322`), Dashboard (`dashboard.rs:1686,1798,1844,1994,2147,2159,2195`),
      Chat (`chat.rs:989,1021,1040`), Cache (`handlers/cache.rs:97,165,252,457`),
      Encryption (`handlers/encryption.rs:210,345`), Autostart
      (`handlers/settings.rs:566,595`), Apps Center (`docker_catalog.rs:490,562`),
      Notifications (`notification_engine.rs:345`),
      Antivirus/Power (`handlers/antivirus.rs:50`, `handlers/power.rs:49`),
      Explorer (`explorer.rs:665,2628,2659`), rclone (`rclone.rs:253,932`),
      DB backup (`db_backup.rs:178,222,229,236,241,270`).
      **Reference implementations already in the tree:** `src/mdns.rs:449`,
      `src/handlers/journal.rs:171`, `src/transfer_quota.rs:136`,
      `src/handlers/docker.rs`, `src/handlers/mcp.rs:588`,
      `src/handlers/syncthing.rs::read_api_key` (2026-10-23) and
      `src/handlers/vm.rs::blocking_io` (2026-10-23).
      *Acceptance:* zero `std::process::Command` / blocking `std::fs` reachable
      from a handler's async body. A CI grep (or a clippy lint) would keep it
      that way.
- [ ] **X.2 — No timeout on subprocesses.** Absent from `disk.rs`, `pools.rs`,
      `encryption.rs`, `cache.rs`, `snapshots.rs`, `rclone.rs`,
      `dashboard.rs`, `health.rs`, `network.rs`, `vpn.rs`, `shares.rs`,
      `explorer.rs`. Present only in Docker, Firewall, Restic, Syncthing, disk
      quotas (2026-10-22) and System Journal (2026-10-23, `run_timed` +
      `JOURNAL_READ_TIMEOUT`) (and `firewall.rs`'s `UFW_TIMEOUT` is the model).
      A wedged `zpool`/`mdadm`/`smartctl`/`df` currently blocks a handler or a
      job forever.
- [ ] **X.3 — Empty `else {}` error handling in the UI.** CloudSync (6),
      Health, Desktop (power), NotificationCenter, Dashboard — *Downloader's 5
      are fixed (2026-10-23: a dismissible `Alert` with the server reason),
      Syncthing's 3 are fixed (2026-10-23: one page-level `Alert`), Explorer's
      11 sites are fixed (2026-10-22, `dlg.notify` + partial-batch messages) and
      the Journal's 5 are fixed (2026-10-23: error state + Retry, in-modal
      errors, a page-level banner)*. Direct violation of
      simplicity rule 5 ("every action gives visible feedback"). A shared
      `notifyError()` helper already exists (`Settings.svelte` uses
      `notifySettings`) — use it.
- [ ] **X.4 — Documented invariants the code does not enforce.** DB-backup
      serialisation (`backup-restic.md:43`), the scheduled-task overlap guard
      (`scheduled-tasks.md`), "RAID never destroys" (`raid.md:3` vs
      `web/mod.rs:952`), the update check being opt-in (`settings.md:33`), the
      single-flight cache mover (`cache.md`), per-user notification read state
      (`notification_repo.rs:10`), "one row per (pool, prefix)"
      (`046_snapshot_schedules.sql:5`, no unique index), metrics gaps
      (`metrics.md:32`). Each is part of the corresponding item above; this line
      exists so the pattern is not lost.
- [ ] **X.5 — `TESTING.md` counts drift where the specs grew.** The Dashboard row
      states "37 runtime tests, recounted on 2026-09-14 via
      `grep -cE '^\s*test\('`"; the same grep on the spec today returns **41**.
      Re-count with `npx playwright test --list --project=firefox` (the
      reference count) and the documented `grep`, never by estimation, and keep
      the dual figure (runtime vs `grep`) whenever a responsive loop makes them
      differ — as the Quotas row correctly does (38 runtime / 36 `test(`).
- [ ] **X.6 — Tests verify shape, not behaviour.** None of the P0 defects is
      caught by the 50+ integration tests around the same code, because they
      assert status codes and `total`. For each item above, the "Tests" line
      names the assertion that would have caught it — add that one first.
- [ ] **X.7 — Two hand-rolled auth checks bypass `require_auth`.**
      `dashboard.rs:2274,2328` (no `expires_at`), and the whole
      `handlers/security.rs` session surface trusts the caller's own token as the
      scope (14.1/14.2). Audit every handler that reads `x-session-token`
      manually instead of going through `admin_guard`.

---

## Process

- Any change to a screen must keep its tests green (Rust + Playwright E2E) —
  see `CLAUDE.md` → "Design system governance". No page is frozen.
- New verification reports go into dated addenda in `CLAUDE.md` (or the relevant
  `docs/guides/` file), not into a separate root file — see `CLAUDE.md` →
  "Keep the repository clean".
- **Before adding an item to this file, verify it against the current code** —
  read the file, do not infer from `CLAUDE.md`, from a guide, or from memory.
  A guide that states an invariant is not evidence that the code enforces it
  (several were false: see X.4). Every item above cites a `path:line` that was
  read in the tree at the revision named at the top of this file; re-check the
  line before quoting it.
- This file must be updated on every bug found-but-not-fixed or incomplete
  feature identified during a future audit, and purged of items that get fixed
  (move the line to "Done" with the date and the guide that carries the detail).
- Order of work: P0 first (data loss, data leak, privacy baseline), then P1 by
  blast radius, then P2. Within an app, always fix X.1/X.2 (offload + timeout)
  before the feature-level findings — they are what turn a defect into an
  outage.

---

## Done

One line per closed item. The detail lives in the named guide, not here.

**2026-10-23 (l)** — Scheduled Tasks — the cron now runs in a real timezone, an
unrunnable task is paused instead of failing every tick, and "Run now" no longer
runs inside the request. **12.2 (UTC-only cron):** `scheduled_tasks` gained a
`timezone` column (`054_scheduled_task_timezone.sql`, `NULL` = follow the
appliance), validated as an IANA name on write (`validate_timezone`, canonical
`Europe/Paris`, `400` naming a bad value) and again at run time
(`parse_timezone`). Evaluation goes through `next_run_at`: the reference instant
is converted into the task's zone, the `cron` crate finds the next local
occurrence, and the result is converted back to UTC — so `0 2 * * *` in
`Europe/Paris` is 00:00 UTC in July and 01:00 UTC in January (both asserted),
and the day-of-week normalisation still holds across the hop. `chrono-tz`
supplies the database; `restic_scheduler`'s parser was extracted as
`parse_schedule` so there is one parser, not two. The appliance zone comes from
`settings::effective_timezone()` (pure in-memory read — no `/etc` access on the
30 s tick), which is what **13.4's timezone half** now makes real: the Settings
value is validated before it is persisted, applied with `timedatectl
set-timezone` through `spawn_blocking` under a scoped sudoers grant, reported
with `timezone_notice` when the apply fails (it used to look like a success),
and seeded from the host at startup on a fresh install. `system_timezone()` is
now one implementation shared with the Dashboard. **12.3 (a bad cron failed
every tick):** `pause_unrunnable` records **one** failed run explaining the
error, then `pause_with_error` disables the task, clears `next_run_at` and keeps
the sentence in `last_error` — the history table can no longer grow one row per
30 s. **12.4 ("Run now" inside the request):** the handler claims the task,
opens the run row, moves the guard **and** the execution into a spawned task and
answers `202` + `{run_id, task_id, status}`; a dropped connection can no longer
release the claim while the child runs (the same task starting twice, the row
stuck on `running`). The child is `kill_on_drop(true)`, `close_interrupted_runs`
closes rows a restart orphaned, and `GET /scheduled-tasks/{id}` is the status
endpoint the caller polls. **Also:** the args field honours double quotes
(`--out "/mnt/my disk/f"` is two argv entries, not three, and re-quotes on
edit), and `POST /{id}/enabled` flips the switch without re-submitting the task,
so pausing no longer wipes the last-run output the row is showing. Tests:
`scheduled_tasks_scheduler.rs` 8 → **12** (`parse_timezone` matrix, winter vs
summer Paris, appliance-zone fallback, unknown zone/cron is an error, weekday
hop), `storage/scheduled_task_repo.rs` 4 → **8** (timezone round-trip,
`set_enabled` keeps the summary, `pause_with_error`, `close_interrupted_runs`),
`handlers/scheduled_tasks.rs` 2 → **4**, `setup.rs` **+1** (the
`timedatectl set-timezone *` grant is scoped), `scheduled_tasks_integration.rs`
6 → **8** (`202` + run id then the row reaches `success`, a failing command is a
run failure, timezone validation/storage, enable toggle keeps the summary),
`tasks.spec.ts` 8 → **11** (timezone field, asynchronous run-now, quoted
argument). Written on 2026-10-23; **not run yet** (per instruction), so the
counts above are the test functions added, not a green run.
`docs/guides/scheduled-tasks.md` gained *Timezones* and the rewritten
invariants; `docs/guides/settings.md` documents the applied timezone;
`docs/API.md` the new routes and the `202`. **Both timezone pickers followed:**
the Settings card rendered a hardcoded 14-zone shortlist while the backend
accepted any IANA name, so an admin in Lisbon or Casablanca could not pick their
zone at all; it now renders `GET /settings/timezones` — every region-prefixed
name `chrono-tz` ships plus `UTC`, sorted, grouped by region. The task form was
worse in the other direction: a free-text field with a `Europe/Paris`
placeholder, where a typo only surfaced on save and the valid names were
invisible. It is now the same grouped picker (with "Server timezone" = the empty
value, which the API reads as `NULL`), rendering the same list through
`frontend/src/lib/utils/timezones.ts` so the two cannot drift apart again; both
keep an unlisted value (a legacy alias set through the API) as its own option so
a select can never silently display, and then save, a different zone.

**2026-10-23 (k)** — Virtual Machines (Incus) — a wait now gets the operation's
own budget, an expired wait is "still running" instead of a failure, and no
blocking filesystem or socket call is left on the runtime. **19.1 (the 45 s
wrapper killed the 900 s create wait):** `request_json` split into
`request_json` (the ordinary `DEFAULT_TIMEOUT` of 45 s) and
`request_json_with_timeout`; `wait_for_operation` uses `wait_budget(secs)` =
`secs + WAIT_GRACE_SECS` (10), so our ceiling always sits **above** Incus' own
`?timeout=` and the daemon's answer arrives first. Its return type is now
`WaitOutcome` (`Completed` | `StillRunning { operation_id }`) instead of
`Result<()>`: an expired wait — or an Incus reply that says "Running" — is
`StillRunning`, **never** `Err`, so the create job completes with
`"…" is still being created — Incus is taking longer than usual (operation …)`
and does **not** delete the instance or trip the TPM self-heal (that branch
keys on `outcome.is_err()`, which is exactly what used to orphan the machine
and duplicate it on retry); `start`/`stop`/`restart`/`delete`/snapshot
create/restore answer `{success:true}` with the same wording and the operation
id, and "still running" after the graceful stop window now triggers the forced
retry instead of skipping it. **Sync replies:** `classify_operation` (pure,
unit-tested) reads the async status codes (200/106 success, 100/101/103/104
still running, 105/107/102/other failure, `metadata.err` wins) and treats a
`"type":"sync"` reply — which carries **no** operation status — as
`Succeeded`; the old `unwrap_or(0)` read the missing code as "the operation did
not complete", i.e. a successful synchronous create reported as failed.
`operation_id` now answers `""` only for a real sync reply and returns a named
`IncusError::Operation` for an async reply with no `operation` field. A
transport deadline is its own `IncusError::Timeout(secs)` ("Incus did not
answer within N seconds"), no longer indistinguishable from an operation
failure. **19.2 (blocking work + a racy port allocation):** every `std::fs`
call in `handlers/vm.rs` — `ensure_iso_dir`, the store listing (`read_dir` + a
`stat` per entry, extracted as `read_iso_entries`), `validate_local_iso` /
`ensure_readable_by_qemu`, `ovmf_secureboot_available` / `swtpm_available` /
`which`, the ISO delete (`is_file` + `remove_file`) and the download's
size-floor + `chmod` (`finalise_iso`) — goes through a new `blocking_io()`
(`spawn_blocking`, the `mdns.rs` / `transfer_quota.rs` shape), and
`start_iso_download` became `async`. `alloc_vnc_display` is `async` and
**claims the slot before it probes it**, both under one lock, so two
concurrent creates can never pick the same display; the claim lasts
`VNC_RESERVATION_TTL` (= `CREATE_WAIT_SECS` + 60 s) instead of a fixed 60 s
that expired mid-create, a rejected candidate is released, and the create job
hands its claim back (`release_vnc_display`) once the machine is up or has
failed. The bind probe itself (`display_is_free`) runs on the blocking pool,
including the `start` path's "this port is taken, move the display" check.
Tests (`src/incus.rs` 13 → **22**: `operation_id` refuses an async reply with
no operation, the `classify_operation` matrix, "a sync reply is never a
failure", `wait_budget` > the operation timeout, and four `/wait` tests over a
throwaway Unix socket — `fake_incus` serves a fixed JSON reply, so `Completed`,
`StillRunning` at the deadline and a real `err` are asserted through the real
transport; `src/handlers/vm.rs` 15 → **21**: three concurrent allocations
never collide, the reservation outlives a create, a bound port reads as taken,
`blocking_io` passes values and messages through, the store lists only
`.iso` files, the 10 MB floor still removes a bad body; `vm_integration.rs`
24 → **26**: the ISO refusals keep their sentences through the blocking hop,
and `GET`/`DELETE /api/vm/isos` list/remove for real; `vm.spec.ts` 43 → **45**
`test(` (47 runtime): section 15, fully stubbed, proves a `start` whose wait
expired shows the server's "still starting … (operation …)" sentence in the
success banner, and that a genuine refusal still shows the server reason).
`docs/guides/vm.md` gained *Timeouts, and what "still running" means* (the two
ceilings table + the `WaitOutcome` contract) and the new display/allocation and
`blocking_io` invariants. **Not run** (per instruction): no unit, integration or
E2E suite was executed; `cargo check --all-targets` and `cargo clippy
--all-targets` are clean for both files. **Left open on purpose:** the two
residual defects the item-19 pass did not touch are now **32.1** (the screen
relay has no framing or heartbeat) and **32.2** (an ISO download is fully
written before the 10 MB floor is checked) in the P2 list.

**2026-10-23 (j)** — Syncthing — a malformed API key is a named error instead of
a dead request task, certificate verification is back on, and the key read left
the async runtime. **18.2 (panic + TLS):** `HeaderValue::from_str(&key).unwrap()`
is gone from `build_headers`; the value comes from `config.xml`, a file an
administrator edits by hand, so it is untrusted input and now goes through
`api_key_header` (`src/handlers/syncthing.rs`), which **trims** the value first
(XML pretty-printing indents the element, and the leading newline alone is an
invalid header value — the most likely way a real config broke), treats a blank
key as "no key configured" (no header, not an error), and returns a message
**starting with `Syncthing API key`** that never echoes the key itself. Every
endpoint answers `{"success":false,"error":"Syncthing API key …"}` (HTTP 200,
module convention) and `handle_syncthing_status` now checks the key *after* the
binary probe and *before* the ping, so a running daemon with an unusable key is
no longer reported as "Installed but not running". `danger_accept_invalid_certs(true)`
is removed from `http_client()`: it was inert only because `SYNCTHING_ADDR` is
loopback `http`, and it would have been a man-in-the-middle the day the address
became configurable; a unit test scans this module's source and fails if the
flag returns (reqwest has no read-back for it). The client keeps its 10 s
timeout and no longer falls back to `.unwrap_or_default()` — that fallback
silently produced a client with **no** timeout. **Also closed in the same pass:**
`find_syncthing_config` (up to four path probes plus a file read) ran inline in
every handler's async body, once per proxied request — it is now
`read_api_key`, on `tokio::task::spawn_blocking` (X.1's `mdns.rs` /
`transfer_quota.rs` shape), and the duplicated GET/POST bodies collapsed into
one `syncthing_request`. **UI:** the three empty `else {}` branches
(`scanFolder`, `toggleDevice`, `restartSyncthing`) became a single page-level
`Alert` (`data-testid="syncthing-error"`, body = the server's own reason) that
also surfaces a status error; while it is shown the "Syncthing is not available /
install it" panel is suppressed, because offering to install an installed
daemon is a lie. Devices/folders *load* failures stay silent on purpose — that is
what the empty state already says. Tests (`src/handlers/syncthing.rs` 11 →
**18** unit: the key→header matrix — realistic key, indented XML, newline/CR/NUL/
DEL, non-ASCII, blank, the `build_headers` matrix, the key never echoed, plus the
source guard for certificate verification; `syncthing_integration.rs` 5 → **8**:
a temporary `config.xml` under `DATA_DIRECTORY` drives the real handlers — a
newline in the key is a named `success:false` error from `/config` (and from
`/status` when the binary exists), an indented key is accepted, a blank key is
"no key" and never a key error; `syncthing.spec.ts` 21 → **23** `test(`: 4.4 a
stubbed unsendable-key status renders the banner **and** suppresses the install
panel, 4.5 a stubbed refused scan surfaces the server reason). The existing
`test_status_for_admin_returns_a_well_formed_envelope` now writes a well-formed
key first, so it no longer depends on whatever `/root/.config/syncthing/config.xml`
holds on the host. `docs/guides/syncthing.md` gained a "REST client (ROADMAP
18.2)" section (the three invariants, the three key rules, the banner contract),
`docs/API.md` the error contract, and the RAG entry for Syncthing now says the
screen explains a configuration fault too. **Not run** (per instruction): no
unit, integration or E2E suite was executed; `cargo check --all-targets`,
`npm run check` and `npm run build` are clean.

**2026-10-23 (i)** — Downloader (aria2) — nothing the request says reaches the
RPC unvalidated, the port is no longer open to any local process, and the
daemon no longer outlives the server. **17.1 (unvalidated `uri`/`dir`/`out`):**
`validate_uri` applies a scheme allowlist (`http`, `https`, `ftp`, `sftp`,
`magnet`) — `file://` (local read as the service user), `data:`, `javascript:`,
`smb:` and every other scheme are refused with a reason; non-magnet links must
carry a host, magnet links an `xt=`. `dir` changed meaning: it is now a
**relative sub-folder of one configured root**, `paths::downloader_dir()` =
`<DATA_DIRECTORY>/downloads` (`validate_subfolder` refuses absolute paths, `..`,
`.` and backslashes), and `resolve_download_dir` canonicalises the deepest
existing ancestor so a symlink planted in the root cannot redirect a download
outside it; `out` must be a bare file name (`validate_output_name`). All three
run **before** `ensure_aria2_running`, so a refused request can never spawn the
daemon, and `dir` is now always sent explicitly so aria2 cannot fall back to its
own cwd. The event log gained `redact_userinfo` (a link carrying `user:pass@` no
longer lands in the journal). **17.2 (no timeout, no secret, fixed sleep):**
each start generates a fresh `rpc-secret` (`hv_` + 128 bits) which is written to
`<DATA_DIRECTORY>/aria2-rpc.conf` with mode 0600 and handed to aria2 through
`--conf-path` — **not** `--rpc-secret`, because `/proc/<pid>/cmdline` is
world-readable (CLAUDE.md "Secrets never hit a command line"); every RPC call
now prepends `token:<secret>` and runs under a 10 s timeout; the 500 ms sleep
became `wait_until_ready`, a bounded `aria2.getVersion` poll (10 s deadline,
200 ms probe) that kills the child if it exits or never answers, so "started"
now means the port is actually up. `src/handlers/activity.rs` dropped its own
unauthenticated RPC re-implementation and uses the shared secret-aware helper
(1.5 s bound for the taskbar). **17.3 (child never reaped, no cleanup, `/tmp`):**
`aria2_is_running`/`ensure_aria2_running` reap a dead child through `try_wait`
instead of reporting `child.is_some()`, `shutdown_signal` in `src/web/mod.rs`
now takes the downloader state and kills+waits the daemon on Ctrl+C/systemd
stop, and the download root moved out of `/tmp/aria2-downloads` (wiped on every
reboot) to `<DATA_DIRECTORY>/downloads`. **UI:** opening the app no longer
spawns aria2c behind the user's back — a stopped daemon shows an explicit
"Start the download service" panel; `add`/`pause`/`unpause`/`remove`/`start`
failures reach a dismissible `Alert` (`data-testid="downloader-error"`) instead
of five empty `else {}` branches; the free folder picker (which offered absolute
paths the API now refuses) became a relative "Sub-folder" field with the
destination root stated underneath; and a task card is a plain container whose
selectable part is a real `<button>` — it used to be `role="button"` wrapping
its own action buttons (WCAG 4.1.2). Tests (`src/handlers/downloader.rs` 15 →
**32** unit — the scheme/sub-folder/out matrix, symlink escape, missing root,
redaction and secret shape; `downloader_integration.rs` 8 → **13** — `dir` under
`DATA_DIRECTORY`, no secret in `/status`, crashed child reaped as
`running:false`, and three refusal tests that assert aria2 was never spawned;
`downloader.spec.ts` 40 → **46** — no auto-start, `file://` and `../etc` refused
with a visible reason, a stubbed failed add surfacing the server reason, a
keyboard-selectable card, and the API refusing `file://` for an admin).
`docs/guides/downloader.md` rewritten around the new invariants, `docs/API.md`
gained the validation table and the `dir` semantics. **Not run** (per
instruction): no unit, integration or E2E suite was executed.

**2026-10-23 (h)** — System Journal — the privilege level now follows the data,
the search runs before the page is cut, and a failed cleanup is retried instead
of being recorded as done. **20.1 (host logs were readable by any session):**
`source_kind` is the single decision — `app` stays session-level (the process
owns those files), `system`/`syslog`/`auth`/`kern`/`daemon` are admin-only, and
an unknown source is a 400 rather than a silent empty page. The gate runs
**before** any subprocess: a non-admin asking for `/var/log/auth.log` gets 403
without a `sudo tail` ever being forked. `GET /journal/sources` advertises only
what the caller may read, so a non-admin sees `["app"]` and the screen never
offers a source the backend will refuse; the desktop hides the app entirely
through `ADMIN_ONLY_APP_IDS` (launcher, global search and dock all read the one
filtered `apps` list). **20.2 (search ran after the line limit):** the filter
now runs over a bounded window — `scan_limit(limit)` = `limit × 20`, hard-capped
at `MAX_SCAN_LINES` = 10 000 — so a page of 10 scans the newest 200 lines and a
rare term older than the page is found. The response gained
`meta.scanned`/`meta.limit`/`meta.search_truncated`, and `Journal.svelte`
renders `journal.search_truncated` above the list instead of presenting an
unexplained short list. Application log reads are bounded too: `read_file_tail`
takes at most the last 8 MiB of each file (the whole-file `read_to_string` had
no cap) and drops the partial line its seek cuts; a file that cannot be opened
is an **error**, not an empty log. **20.3 (a failed cleanup recorded itself as
done):** the tick is now the pure `plan_tick` (`Disabled | Baseline | Due |
NotDue`) and `last_cleanup_at` advances **only on success** — a failure returns
`Err`, the loop logs it, and the next hourly tick retries instead of waiting out
a whole `interval_days`. The never-run branch still only writes a baseline
(cleaning there wiped the current day's audit log on every restart). Residuals
closed in the same pass: `run_timed` bounds every `journalctl`/`tail` call
(`JOURNAL_READ_TIMEOUT` 20 s, child killed on expiry) and drains both pipes on
their own threads — a 10 000-line dump overflows a pipe buffer, and a child
blocked on `write` never reaches `try_wait`, so the deadline could not fire at
all; a 4-slot `READ_PERMITS` semaphore caps concurrent forks;
`validate_unit_filter` answers 400 naming the accepted characters instead of
surfacing raw `journalctl` stderr (the value was always a single argv entry, so
never an injection); `clear_app_logs` collects every per-file failure
(`clear_outcome`) instead of returning on the first one, so a partial clear can
no longer look like a success; and the UI's five empty `else {}` branches became
an error state + Retry, in-modal errors that keep the dialog open on failure,
and a page-level banner, with Clear/Auto-cleanup gated on `$isAdmin`. Tests
(`src/handlers/journal.rs` 9 → **20** unit, `src/journal_cleanup_scheduler.rs`
0 → **4**, `journal_integration.rs` 11 → **17**, `journal.spec.ts` 27 → **30**
`test(`); `docs/guides/journal.md` rewritten (per-source privileges, the read
bounds table, failure semantics, the corrected truncate-vs-unlink invariant),
`docs/API.md` gained the per-source auth table and the `meta` contract. **Not
run** (per instruction): everything above compiles (`cargo check
--all-targets`, `npm run check` clean) but no unit, integration or E2E suite was
executed.

**2026-10-23 (g)** — LUKS2 encryption — the three ways it could destroy data or
wedge itself are gone, and the two operations that did not exist exist.
**9.2 (erase gate):** `DELETE /encryption/volumes/{id}` no longer runs
`let _ = unmount(); let _ = close(); erase();`. It unmounts (a failure answers
400 and stops), closes the mapper if `cryptsetup status` says it is open, then
**re-verifies** the state from the kernel's own view — `/proc/self/mounts`
(parsed by `is_mounted_in`, with `\040`-style octal escapes decoded and the
mount point matched as a whole field, so `/mnt/encrypted/vault2` cannot satisfy
`/mnt/encrypted/vault`) and `cryptsetup status` — and only then erases;
`erase_gate(mounted, open)` is the single decision, and an unreadable
`/proc/self/mounts` fails **closed** (refuses the erase). Every refusal keeps
the metadata row, so the volume stays listed and fixable. **9.3 (unlock
rollback):** a successful `open()` followed by a failed `mount()` now closes the
mapper again; when even that fails the message names the mapper and the
`sudo cryptsetup close` line to run (`unlock_failure_message` has both
branches under test). **9.1 (create is a job):** `POST /encryption/volumes`
validates synchronously (passphrase policy, filesystem allow-list, label,
duplicate label, then the device gate — nothing privileged runs on a refusal)
and answers `{ job_id }`; the work runs as `Custom("encryption_create")` with
milestones 5/30/45/70/80/88/95 %, serialized behind one `VOLUME_LOCK`. A
failure after `luksFormat` rolls the container back (unmount → close → erase)
so the device returns to the available pool instead of being stuck "in use"
with no metadata row, and a rollback step that fails is named in the error
rather than swallowed. `format_and_open` was split into `format_luks` + `open`
so the job knows whether a container exists before deciding to erase. Also
closed in the same block: **passphrase rotation** exists —
`POST /encryption/volumes/{id}/passphrase` + `encryption::change_passphrase`,
which sends the three lines `luksChangeKey` actually prompts for
(`change_key_stdin`; the old shape sent one, so every call would have failed on
"Passphrases do not match") and then proves the new passphrase with
`cryptsetup open --test-passphrase`, refusing a no-op rotation; **passphrase
policy** `MIN_PASSPHRASE_LEN = 12` + no wordlist entry + no label reuse + at
least four distinct characters, applied to new passphrases only so existing
volumes stay unlockable; **lock** reports the unmount failure instead of
closing anyway; `write_file_as_root` no longer `.unwrap()`s the child's stdin;
the fstab edits are async and their pure halves (`with_marked_entry` /
`without_marked_entry`) are unit-tested; the list probes each volume's status
**concurrently** instead of serially; and `wipefs -a`'s failure is reported
(`EraseOutcome.warning`) instead of leaving the device permanently unusable
through the UI with no explanation. Every privileged call is bounded
(`run_timed` with `kill_on_drop`: format 900 s, open/`luksChangeKey` 180 s,
close 120 s, erase/`wipefs` 600 s, status 15 s, UUID 30 s, mount 120 s, fstab
60 s). The progress stream now replays a job's last known event on subscribe, so
a create job that fails before the modal connects (a refused `sudo`, a missing
tool) still delivers its reason instead of leaving the panel on its bar. Tests
(`src/encryption.rs` 2 → **24**, `handlers/encryption.rs` 0 →
**6**, new `tests/encryption_integration.rs` **15**, `progress_integration.rs`
8 → **9** (the SSE replay), `storage_integration.rs`
updated to the new policy, `storage.spec.ts` +2 cases and the opt-in 13.1
rewritten to follow the job and prove the rotated passphrase);
`docs/guides/encryption.md` rewritten (job, gate, rollback, rotation, policy,
deadline table, and the false "`luksChangeKey` has no caller" claim removed),
`docs/API.md` gained the rotation route and the `{ job_id }`/`warnings`
contracts. **Not run** (per instruction): everything above compiles
(`cargo check --all-targets`, `npm run check` clean) but no unit, integration
or E2E suite was executed.

**2026-10-23 (f)** — RAID wizard — a failed setup can no longer destroy a
device it never assembled. **8.1:** `teardown_failed_array` now decides from an
identity captured right after *its own* `mdadm --create` (`mdadm --detail` →
label + UUID + members, parsed by the new `parse_mdadm_detail`) instead of
trusting the request: it zeroes only the intersection of (request members) ×
(devices the kernel reports as members **now**), wipes nothing when the
identity is unknown, refuses to touch a node whose label/UUID changed (a
concurrent job recreated it), and requires `mdadm --stop` to succeed before any
superblock is erased — a failed stop means the array is still live, typically
still mounted, which is exactly the case when the failure happened *after* the
mount. A non-zero exit from `--zero-superblock` is now reported instead of
being mistaken for a spawn error, and a failed `mdadm --create` goes through the
same teardown (a create that dies on the fourth device used to leave three
superblocks pointing at an array nothing would assemble). **8.2:**
`wait_for_array_ready` returns `Result`: the ceiling went 20 s → 120 s
(`ARRAY_READY_TIMEOUT`), a `clear` `array_state` fails immediately (the kernel
holds no array — polling cannot fix it), and running out of the deadline fails
the job and tears the array down instead of running `mkfs` + `mount` on a device
the kernel is still assembling. **8.3:** every privileged call of the create
path goes through one `run_timed` helper (`spawn` + `kill_on_drop` +
`tokio::time::timeout`, so a timed-out tool really dies) with the deadlines
`MDADM_TIMEOUT` 120 s, `MKFS_TIMEOUT` 900 s, `MOUNT_TIMEOUT` 60 s,
`BLKID_TIMEOUT` 30 s, `ZPOOL_TIMEOUT` 120 s, `INITRAMFS_TIMEOUT` 300 s; a
timeout is the distinct `DiskError::Timeout` ("did not finish within Ns")
because the tool may have been doing the right thing. Also closed in the same
block: the degraded-create policy is **one** table (`max_missing_for_level`)
read by both the API gate and the job, so raid0/raid10 refusals now name their
reason ("no redundancy" / "mdadm only places a missing slot in RAID 10 with an
explicit layout") instead of only a slot count — the raid5/6-vs-raid10
difference is deliberate and documented rather than incidental; and the ZFS
branch dropped its `-f` (matching the decision taken for `create_zfs_pool` in
7.4): a device carrying a pool signature is a conflict the free-device gate
already reports, never one to overwrite. The `ntfs` drift named by the item was
already closed in 7.4. Tests (`src/disk.rs` 42 → **66**, `tests/raid_integration.rs`
5 → **6**): `parse_mdadm_detail` (standard body, `removed`/`missing` rows never
become members, no `Name :` line → `None`), `plan_teardown` × 6 (only this job's
members, foreign UUID, renamed node, unknown identity → stop-only, array gone,
label fallback), `wipe_targets`, `classify_array_state`, `resync_percent`, four
`wait_for_array_state` cases against a temp sysfs (becomes active, resync percent
mapped into the 10-40 band, `clear` fails fast, silent kernel times out),
`run_timed` (fast command, hung command killed at the deadline), the policy
table, the refusal wording, a test proving both validators return the same
message, and three `zfs_create_args` cases (pool name before the vdev type,
no `-f` on any level, one `mirror` group per pair for raid10); the
integration test drives `POST /api/storage/raid` with a degraded raid0 and a
degraded raid10 and asserts the 400 names the reason and starts no job.
`docs/guides/raid.md` gained the "What is *not* covered by tests" section the
item asked for; `docs/guides/storage.md` needed no edit (its `-f` statement is
about `create_zfs_pool`, which already lost it in 7.4). **Not run** (per
instruction): the tests above were written and compile
(`cargo check --lib --tests`), `cargo fmt` is clean, but no unit, integration
or E2E suite was executed.

**2026-10-23 (e)** — Storage — the residual privileged call sites are fixed and
the rest of the sudoers file is scoped. **7.4:** (a) `disk.rs`'s four
mount/unmount helpers ran bare `mount`/`umount` and `mount_disk`/
`unmount_disk` discarded the exit status, so a kernel refusal was reported as
success — they had **no caller anywhere** in `src/` or `tests/`, so they were
removed instead of fixed; `create_zfs_pool` now runs `sudo zpool create`
**without `-f`** (a device carrying an exported pool is a conflict to surface,
not to overwrite); `format_device` lost its `ntfs` → `mkfs.ntfs` arm (no grant,
and the wizard never accepted it) so it fails loudly as unsupported;
`list_removable_disks` parses `lsblk -P` through the existing
`parse_lsblk_pairs`, so a label with a space no longer shifts every column;
`is_removable_transport` dropped `ata`/`sata`/`scsi`/`nvme`, which had made
every internal disk appear under "removable media"; `get_smart_details` and
`get_disk_temperature` read SMART rows through one pure parser
(`parse_smart_attribute`) anchored on the TYPE column (`Pre-fail`/`Old_age`)
instead of `parts[9]` (which is WHEN_FAILED, i.e. "-" read as 0, whenever the
name spanned two tokens, and which also mis-read a `38 (Min/Max 20/45)`
RAW_VALUE); `get_smart_test_status` no
longer invents `pct * 2 / 10` minutes — it uses the drive's own advertised
duration for that test kind from `smartctl -c` (`parse_smart_polling_minutes`,
which reads the kind on the line *above* the polling time); and
`mount_and_persist_raid` reports a failed/empty `mdadm --detail --scan` or a
failed `mdadm.conf` append at `error!` level with the consequence spelled out
("will not reassemble after a reboot", plus the line to add by hand) instead of
dropping both results. (b) every remaining `zpool`/`btrfs`/`mdadm` read goes
through `sudo`:
`handlers/dashboard.rs` (`mdadm --examine`, `zpool status`, `zpool list` — they
silently reported "no array"/"no pool" for a non-root service), its dead
`btrfs filesystem show` block deleted, and `disk.rs::detect_disk_role` plus
both `gather_device_facts` paths, where a bare read meant `zfs_member`/
`btrfs_member` were always false — a ZFS/Btrfs member could therefore be
offered as a free disk for a new array. `detect_disk_role` also passes a
`/dev/<name>` vdev path to `zpool status` (a bare device name never matched).
The USB `-d sat` SMART retry now puts `-d sat` first so its argv matches a
grant. (c) The rest of `configure_sudoers` is scoped the same
way as 7.1: a second table (`HostBins`) renders the VPN, interface, firewall,
`mdadm`, `smartctl`/`journalctl`/`tail` lines — `wg` only `wg0`,
`openvpn --config /etc/openvpn/*.ovpn … --daemon`, `ip` per object (never
`netns`/`tunnel`/`rule`), one `wpa_cli` grant per subcommand, `ufw` per
subcommand, `mdadm` per mode (both `--action` spellings), `smartctl` per flag
with the device confined to `/dev/*`, `tail` to the four whitelisted logs;
`dhclient` (used by `ip_sudo` but never granted) and `exportfs -ra` are now
covered, and `pkill` — resolved and granted but never run — is gone.
`vpn.rs` passes an explicit `-TERM` to match the pinned `kill` grant. Still
open by design: sudoers cannot express a numeric pid (`kill -TERM *` is a
documented residual, same class as `chpasswd`), and command timeouts are 8.3,
not here. `src/setup.rs` +12 tests (bare-grant/shell guards, `pkill` gone,
`wg0`/openvpn/config-dir pinning, `ip` objects, `wpa_cli`/`ufw` per
subcommand, both `mdadm --action` spellings, `/dev`-confined `smartctl`,
whitelisted `tail`, every host binary represented, and no grant may carry an
unescaped comma — sudoers reads one as a command separator, which is why the
journalctl `--output-fields` list is written `\,` — 73 → 85); `src/disk.rs` +8
(`is_removable_transport`, `lsblk -P` label-with-space and empty-field rows,
`parse_smart_attribute` standard/two-token-name/header rows and a
`(Min/Max …)` RAW_VALUE,
`parse_smart_polling_minutes` per kind — 34 → 42). (`docs/guides/storage.md`,
`docs/guides/raid.md`, `docs/guides/vpn.md`, `docs/guides/network.md`,
`docs/guides/firewall.md`, `docs/guides/journal.md`, `docs/ARCHITECTURE.md`.)

**2026-10-23 (d)** — Storage — the sudoers grants are scoped. **7.1:** the
Storage/LUKS block in `SetupService::configure_sudoers` granted `zpool`, `zfs`,
`mkfs.btrfs`/`ext4`/`xfs`, `mount`, `umount`, `wipefs`, `btrfs`, `cryptsetup`,
`blkid`, `mkdir`, `rmdir` and `tee` with **no argument restriction**, so any
attacker-controlled string reaching an argv (10.1, 17.1) became
`mkfs.ext4 /dev/sda`, `zfs destroy -r tank` or `tee /etc/shadow` as root. The
block is now rendered from a small table (`StorageBins` →
`pool_grants`/`filesystem_grants`/`mount_grants`/`config_grants`) where every
entry carries its scope: `zpool`/`zfs`/`btrfs`/`cryptsetup` per subcommand,
`mkfs.*`/`wipefs`/`blkid` per flag shape, and the shared host-wide tools per
path — `mount * /mnt/*`, `umount /mnt/*`, `mkdir -p /mnt/*`, `rmdir /mnt/*`,
`tee /etc/fstab`, `tee /etc/mdadm/mdadm.conf`. The SFTP jail root
(`/var/lib/homevault-sftp/…`) and removable media (`/media`, `/run/media`)
keep the grants they were relying on implicitly, and the other modules that
had been covered by that `tee` (resolv.conf, netplan/ifupdown, avahi, Docker's
`daemon.json`, the cpufreq governor, `/proc/sys/net/ipv6/conf/*/*`) got their
own exact-path grant so nothing broke — including the `rm -f` that unpublishes
the avahi service file, which had no grant at all. `mkfs.exfat` — Explorer's
"format as exFAT", already a tracked dependency — had no grant at all and is
now covered. What sudoers cannot constrain is documented in place: the pool
name and options after a `zpool`/`zfs` subcommand stay variable, which is the
feature. `src/setup.rs` gained 12 tests over the rendered table (no grant may
be a bare binary or reach a shell; the per-subcommand/flag/path scopes are
pinned), and the residual call sites the scoping exposed are now their own
finding (7.4 — closed in (e) below). (`docs/guides/storage.md`,
`docs/guides/encryption.md`, `docs/guides/raid.md`.)

**2026-10-23 (c)** — Storage — scrub works, destroy is unmount-gated. **7.2:**
"Scrub" on an mdadm virtual disk ran `sudo bash -c "echo check >
/sys/block/<md>/md/sync_action"`; `bash` is not (and should not be) in the
sudoers grant, so the action always failed with a sudo prompt. It now runs
`sudo mdadm --action check <array>` — the same call `rebuild_raid` already makes
on a healthy array, on a binary the RAID page is already granted, so no new
privilege and one implementation instead of two. **7.3:** `delete_raid` used to
discard the `umount` result and then run `mdadm --stop` +
`--zero-superblock` on every member, erasing the metadata of a filesystem that
was still mounted and being written to. It now resolves every mount point
backed by the array from `/proc/mounts` (canonical kernel node,
`/dev/md/<name>` symlink and `UUID=…` source all match), unmounts each one and
**aborts on the first failure** — before the stop, the `mdadm.conf`/`fstab`
cleanup and the superblock wipe — with a second, kernel-side `umount <device>`
check that only tolerates "not mounted". The Storage destroy confirmation is
now typed (`ConfirmDialog` `requireText` → `dlg.ask(msg, { requireText })`
keeps the danger button disabled until the disk name is typed exactly, label
`common.type_to_confirm` ×4 locales), and the legacy
`DELETE /api/storage/raid/{name}` + `handle_raid_delete` are gone: they had no
UI caller, duplicated the typed Storage path, and contradicted
`docs/guides/raid.md` — the RAID surface is creation-only again, so doc and
route agree. No test was added in this pass (explicitly out of scope); the
existing suites keep their counts (`tests/raid_integration.rs` 5 — the DELETE
entry left the write matrix without changing the test count,
`tests/e2e/storage/storage.spec.ts` 43). (`docs/guides/storage.md`,
`docs/guides/raid.md`, `docs/API.md`.)

**2026-10-23 (b)** — Apps Center / app-proxy — the last P0. **6.1:** the
minutes-long compose calls left the async runtime —
`start_project`/`stop_project`, `create_project` and the dependency gate's
per-app `get_project` probes (which shell out to `docker compose` once per
catalog entry) now run on the blocking pool, so a first install no longer pins
a worker while it pulls images; the taskbar job still wraps the whole thing.
**6.2:** an app session is a per-app grant — `APP_AUTH.mint(user, ns_id)`,
`validate(token, ns_id)`, and `revoke_app(ns_id)` on stop (`revoke_all` is
gone). Stopping or uninstalling one app no longer signs the household out of
every other proxied app. **6.3:** both redirects of the round-trip (the gate's
challenge and `/app-auth`'s answer) resolve their host through the new
`app_proxy::origin`: the `Host` header is normalised and honoured only when it
names the appliance (`HOMEVAULT_HOST`, `<host>.local`, `localhost`, loopback,
the primary LAN address, or a `HOMEVAULT_TRUSTED_HOSTS` entry) and otherwise
falls back to the canonical host, so a forged header can no longer aim a
cookie-carrying redirect somewhere else; IPv6 authorities are bracketed.
Also closed in the same block: `write_install_marker` reports its failure
instead of discarding it (the marker is written *before* compose starts, and a
failure after startup stops the app again rather than leaving a state the UI
cannot describe), `POST …/proxy` answers `applied` and errors on an
unpersistable marker instead of reporting success, and the install/update
paths' `std::fs` work moved off the runtime. Tests: `app_proxy::origin` **8**
(new), `app_auth` 6 → **8**, `serve` 4 → **5**, `handlers/docker_catalog.rs`
**0 → 4**, `tests/app_proxy_integration.rs` gains three steps (forged `Host`,
cross-app token refusal, stop-one-app scope). (`docs/guides/apps-center.md`,
`docs/guides/docker.md`, `docs/API.md`, `docs/BACKEND.md`,
`docs/ARCHITECTURE.md`, `TESTING.md`.)

**2026-10-23** — AI Assistant / RAG — the knowledge base has a user-facing
surface and an integrity contract. **4.1:** the computer-visible endpoints
became a product feature — `KnowledgeBaseCard` (Settings → AI Assistant) shows
the index status (documents, chunks, `idle`/`indexing`/`error`, last indexed),
installs the embedding model, triggers a reindex and polls progress every 2 s;
the Chat empty state names an empty index (with an "index now" action for an
admin) instead of the assistant silently answering "I don't know" for shipped
features; `GET /rag/status` always reports the required embedding model, even
before the first index, so the card can gate the action on it. **4.2:** a
reindex now **reconciles** — each indexer returns the ids it produced and
`rag::prune_source` drops every document of that source the run did not (a
deleted guide/renamed app no longer stays searchable forever; `clear_source` is
now this with an empty keep set and finally has a caller). An embedding failure
**fails the run loudly**: `upsert_document_with_chunks` embeds before writing
and returns the error, no chunk is stored with a `NULL` embedding, and
`index_all` records `error` instead of warn-and-continue; `last_indexed_at` and
the counts only advance on a completed run, so a failed reindex cannot report
"last indexed: now" with zero documents. A second reindex while
one runs is refused (`ReindexGuard`). `GET /rag/documents` and `/rag/search`
report an error rather than an empty result. The indexers take a new
`EmbeddingProvider` (`OllamaEmbedder` in production, a deterministic fake in
tests). **4.3:** `detect_system_specs` (nvidia-smi/rocm-smi + `/proc/meminfo`)
runs on the blocking pool, and the Chat RAG failure is logged, not swallowed.
The `/chat/*` error strings and the pull success message are English now (they
leaked French into every locale; the recommendation copy is still French-only —
known gap noted in `docs/guides/chat.md`).
Tests: `rag.rs` 10 → 15 unit, `rag_indexers.rs` 3 → 5 unit,
`tests/rag_integration.rs` (8, new), `tests/e2e/settings/knowledge-base.spec.ts`
(4, new). (`docs/guides/rag.md`, `docs/guides/chat.md`, `docs/API.md`.)
**Still open from this area, deliberately not in this pass** (none blocking, none
counted above): search loads every chunk and runs cosine in the app process
(a real index would need a vector store); the assistant renders hand-rolled
markdown through `{@html}` with no sanitizer; conversations live only in
`localStorage`; Ollama availability is probed once per page load; and
`system.reboot` remains reachable as an MCP tool.

**2026-10-22 (h)** — Disk quotas — the home-directory size limit now actually
limits. Every save resolves the backend from the **mount that holds the folder**
(`findmnt`, not the disk label in the DB), applies the limit (`setquota -u` on
ext2/3/4/xfs/jfs/reiserfs, `zfs set userquota@<user>` on ZFS) through
`sudo -n` under a 20 s deadline, then **verifies it with a `repquota`/`zfs get`
read-back** and reports the read state, so a limit the kernel refused is never
shown as applied. Eligibility is created, not assumed: RAID/pool mounts take
`usrquota,grpquota` (persisted to `/etc/fstab` only when the live mount accepted
them, plain-mount fallback otherwise), the installer edits the `/home` fstab
line in place, verifies it with `findmnt --verify --fstab`, rolls back on
failure, remounts, then runs `quotacheck` + `quotaon`, and `activate_all()` runs
`quotaon -aug` at startup so a reboot cannot silently switch enforcement off.
Btrfs and non-POSIX filesystems are reported **unsupported**, never "applied".
`GET /users/{u}/home` carries a live `quota` object (`enforced`, `filesystem`,
`mount_point`, `limit_bytes`, `used_bytes`, `reason_code`, `reason`) and the
Users home card renders exactly two outcomes — enforced with usage, or not
enforced with a localised cause (4 locales) — refilled from a read-back after
every save instead of echoing the form. Tests: `quota.rs` 2 → 21 unit,
`tests/quota_integration.rs` (8, new), `users.spec.ts` 82 → 94 (§18)
(`docs/guides/quotas.md`, `docs/guides/users.md`, `docs/API.md`,
`docs/BACKEND.md`, `docs/ARCHITECTURE.md`, `docs/guides/raid.md`).

**2026-10-22 (g)** — Scheduled Tasks — the overlap guard can now fire. `Run
now` built a **fresh empty** running set on every call, so the "already
running" test could never be true: a manual run started while a scheduler tick
was executing the same task double-ran it (two `restic`/`clamscan`/`rsync` on
one target). Both paths now share one process-wide `RUNNING` set behind a
`TaskRunGuard` released on drop — a panic or an early return cannot wedge a
task forever — and the guard is claimed before `execute`, so the tick skips
(logged, row untouched) and `POST /scheduled-tasks/{id}/run` answers `409`
`Task '<name>' is already running`. Also closed: the doc comment and
`docs/guides/scheduled-tasks.md` claimed the guard already worked; the guide
now describes what the code does. Tests: 2 unit (refusal + registry shared by
both paths, both directions) and 1 integration (a `/bin/sleep 1` run holds the
task, the concurrent second POST is `409`, exactly one run row, the claim is
released so the next start succeeds) — the single missing test named by 12.1
(`docs/guides/scheduled-tasks.md`, `docs/API.md`).

**2026-10-22 (f)** — Snapshot schedules — a run now reports what actually
happened. A failed prune is a **failed** run (`last_status = "failed"` with the
reason, plus one `snapshot.schedules` notification per outage, not per tick)
instead of a green row while the pool fills up. Runs are single-flight per
schedule **and** per pool (`RunGuard` over a process-wide registry, released on
drop), so a "Run now" cannot overlap a scheduler tick — refused with `409` and
the row untouched. The backend is chosen per **pool** (`SnapshotService::kind_for`
over `PoolService::scan_pools`; mdadm arrays resolved by the filesystem on top)
instead of "is the `zfs` binary installed", which sent every Btrfs pool down the
ZFS branch on a mixed host. Also closed: GFS tier windows derive from the
configured counts (`keep × period`, clamped monotonic) so "keep 48 hourly" and
"keep 8 weekly" mean what they say; the `auto` badge requires one of the pool's
own schedule prefixes (a manual `mydata-20261021-030000` is no longer tagged
`auto`); a schedule on a nonexistent pool is refused at create/update;
`(pool, prefix)` is unique (migration `053_snapshot_schedule_unique_prefix.sql`,
`409` on conflict); `GET /snapshot-schedules/snapshots` is `require_auth` like
every other Storage read; and the Storage Snapshots tab surfaces a refused or
failed run instead of ignoring the response (`docs/guides/snapshots.md`,
`docs/guides/notifications.md`, `docs/API.md`).

**2026-10-22 (e)** — Cache tier — the mover can no longer fill the array and
then delete its sources: a destructive move first proves the destination can
hold the payload (`find`-sized, `df`-checked, age filter applied; refusal means
rsync never runs, so nothing is deleted), every share yields a persisted
per-share outcome (`cache_mover_run_shares`, migration 052) and one failure no
longer aborts the run (status `completed | partial | failed`, the summary names
the shares that were not moved), and pool mounts are resolved by one
`PoolMounts::scan` per run/request with the scan on the blocking pool and a
distinct "failed to scan pools" error. Also closed: `cache_pool: ""` clears the
designation, share update validates `target_pool`, `min_free_percent` is 1–95
server-side, a failed scheduled run no longer advances `mover_last_run_at`
(15-min retry backoff), and a restart closes the stale `running` row
(`docs/guides/cache.md`, `docs/API.md`).

**2026-10-22 (d)** — Settings — the update check is opt-in and enforced
server-side: `Manual`/`Disabled` never contact the update endpoint, an opted-in
schedule fires once per interval, and only the new admin
`POST /settings/version/check` forces a check (`docs/guides/settings.md`).

**2026-10-22 (c)** — Account / Security — self-service session list no longer
returns live session tokens (opaque 12-hex SHA-256 `id`), revocation is
caller-scoped (ids resolve only against `find_all_by_username(caller)`; a
foreign id/token is a 404), the current session stays protected, and the two
caller-less avatar helpers were deleted (`docs/guides/security.md`,
`docs/guides/account.md`, `docs/guides/avatars.md`).

**2026-10-22 (b)** — File Explorer — the four blocking findings closed: zip-slip
proof extraction (pre-flight member validation in the C locale, symlink-member
archives refused, staging dir + confinement verification before merge, `.gz`
streamed to disk), the SSE watcher authorised (tickets bound to user + path,
component-prefix event filter, RAII watch release), resumable upload sessions
owned and capped (403 cross-user, 429 above 20 per user / 256 global), admin
delete confined by the PathGuard and refusing a browsing root — plus the `--`
argv guard in `compress_items`, the eleven silent `else {}` failure branches
reported via `dlg.notify` (19 i18n keys ×4 locales), and the stale guide claims
corrected. Non-blocking residuals tracked as **10b** (TOCTOU admin-path
canonicalisation, chunk RAM buffering, `SHARE_SEMAPHORE` unwrap)
(`docs/guides/explorer.md`, `docs/API.md`).

**2026-10-22** — Health: off-runtime checks (`run_all_checks`/`run_check`
async + self-offloading), one shared sudo-wrapped `smartctl` helper used by
Health / Dashboard / Storage, real account-store check, the appliance's own
systemd unit derived from the cgroup and monitored, link state from
`operstate`, per-check partial degradation + explicit error/retry state in
the UI (`docs/guides/health.md`).

**2026-10-21 — nine production-ready passes**

| Area | What changed | Detail |
|---|---|---|
| Firewall / ufw | argv validation, spec-based delete, verified enable, default policy + change history | `docs/guides/firewall.md` |
| Backup / restic | backup and restore became background jobs with real streaming progress, argv validation | `docs/guides/backup-restic.md` |
| Users / Groups | host account lifecycle, member validation, forced first-sign-in change, session audit | `docs/guides/users.md`, `docs/guides/groups.md` |
| Rescue | real cancellation, confined reads, clone safety net | `docs/guides/rescue.md` |
| Shares | config-injection guards on every field, line-exact NFS exports, blocking pool | `docs/guides/shares.md` |
| Network | persisted config applied, verified and rolled back via netplan/ifupdown | `docs/guides/network.md` |
| APPS / app-proxy | app-proxy auth survives HTTPS deployments | `docs/guides/apps-center.md` |
| VPN | OpenVPN start, profile deny list, cached public IP | `docs/guides/vpn.md` |
| Docker (2026-10-20) | volumes, prune, inspect/exec/pause/rename, live logs, builds, registries, `daemon.json`, update-to-newest | `docs/guides/docker.md` |

**2026-10-19** — RAID creation hardened and wired to the background-job system
(20 findings, `docs/guides/raid.md`); 9th health check (`raid_arrays`).

**2026-10-13 (c)** — OMV-inspired priority-1 features: Scheduled Tasks, Metrics,
host Power + disk spindown, per-event notification toggles + webhook sinks,
snapshot schedules with GFS retention (migrations 043–046).

**2026-10-12** — user deletion revoking share permissions; `get_progress`
placeholder; `JobType::Custom` serialization; Cloud Sync config path;
RAG guide coverage enforced by test; three `manual_contains` clippy warnings.

**2026-09-07 → 2026-09-14** — the live "real host conditions" passes (Firewall,
Shares/SFTP, RAID/Storage, System journal, Rescue, LUKS, Network), transfers v2
(HTTP `Range`, short-lived tickets, resumable chunked upload, quota locks,
`PathGuard`, trash, rate-limited share links), and the desktop chrome / topbar /
app-window alignment to the approved mockups.

**Earlier** — the 2026-07-21 and 2026-09-05/06 passes. Full history:
`CHANGELOG.md`, `TESTING.md`, and the dated addenda in `CLAUDE.md`.
