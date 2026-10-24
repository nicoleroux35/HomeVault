# HomeVault Roadmap

Last updated: 2026-10-24 — current version: 1.0.0

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
**Downloader**, **Syncthing's REST client**, **Virtual Machines (Incus)**,
**Scheduled Tasks** — timezone-aware cron, pausing an unrunnable task, and
"Run now" off the request — and **Transfer quotas** — failing closed, a
calendar counting period, and an admin-gated read, and finally **Metrics** —
nullable series and elapsed-time rates, **Dashboard / Widgets** — per-account
tiles, deadlines on every host probe, and no fabricated measurement, and
**Syncthing's folder/device management** — a share or a peer can now be onboarded
from HomeVault, with every field validated before the daemon is called (see
Done).

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
| 16.4 | Cloud Sync (rclone) — no scheduler | P2 | 1 |
| 26 | Wallpapers / Themes | P1 | 2 |
| 27 | Web shortcuts | P1 | 2 |
| 28 | Activity taskbar | P2 | 3 |
| 29 | mDNS | P2 | 2 |
| 31 | Docker — `api.ts` consolidation | P2 | 1 |
| 32 | Virtual Machines — residual | P2 | 2 |
| 33 | Transfer quotas — residual | P2 | 1 |
| — | Cross-cutting (7 systemic) | — | 7 |

Total open findings: **24** (17 per-area + 7 cross-cutting).

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
now" answers `202` while the run continues outside the request. **23 (Transfer
quotas) closed on 2026-10-23**, and with it 30.1: the guard now fails **closed**
(a quota that cannot be loaded refuses the transfer with `503`), the counters
are counted over a real calendar day/week/month window settled at check time
*and* by a reset loop, and both read endpoints enforce self-or-admin instead of
trusting a screen-side redirect. That was the last P1 that let a request
through when the guard itself could not answer. **11 (Metrics) closed on
2026-10-23**: every series is nullable and a failed `/proc` read is a gap
instead of a `0.0`, each series averages on its own, throughput divides by the
real elapsed time (and the loop skips missed ticks instead of bursting them
into one bucket), and `latest` answers `200` + `data: null` on a fresh install.
**15 (Dashboard / Widgets) closed on 2026-10-23**, and **16 (Cloud Sync / rclone)
closed the same day**: every rclone subprocess runs under a deadline, a started
mount is owned by this process and unmounted at shutdown, the credential file is
written `0600` and edited as parsed sections, and the screen no longer swallows a
failure. Only the missing *scheduler* is left, as P2 **16.4**. **13 (Settings)
closed on 2026-10-23** too (see Done), and **21 (Notifications) closed the
same day** — see Done (2026-10-23 (r)): read state is per user, the feed is
paginated, and the shared delete paths are admin-only. **22 (DB backup) closed
on 2026-10-24** — see Done: the snapshot lock is process-wide, a failed attempt
keeps its reason for the card, and retention no longer eats the pre-migration
restore point. **18 (Syncthing) closed on 2026-10-24**: 18.1 was the last
finding, and it was the one a user hit first — a share or a peer could not be
onboarded from HomeVault at all. **24 (Server control) closed on 2026-10-24**:
a refused `restart`/`shutdown` now says why, a successful one says it is going
down, and every power button (desktop, Settings, MCP) runs one bounded
`sudo systemctl` primitive. **25 (Autostart) closed on 2026-10-24**: every
`systemctl` call is off the async runtime with a deadline, a host that cannot
answer is no longer reported as "disabled", and a refusal carries the host's
own reason. The next P1 is **26 (Wallpapers / Themes)**, where a non-admin
never receives the server appearance and `theme`/`density` are persisted
no-ops.

---

## P0 — Broken

None. Item 6 (Apps Center / app-proxy) was closed on 2026-10-23 — see
"Done".

---

## P1 — Needs work

### 16.4. Cloud Sync (rclone) — a persisted sync job never runs on its own

16.1–16.3 were closed on 2026-10-23 — see Done. One "also" item was left open
on purpose, because it is a new feature and not a defect in what shipped:

- [ ] **16.4 — No scheduler.** `POST /rclone/jobs/{id}/run` is the only way a
      persisted job runs. The `sync_jobs` table stores the definition only — no
      due time, no interval, no last outcome — so a nightly sync cannot be
      expressed, and the screen's "last run / status" columns are the outcome of
      the click that just happened, not a server-side run state (commented as
      such at the call site so it is not mistaken for one).
      *Acceptance:* an interval or cron column (the Scheduled Tasks scheduler
      already has `compute_next_run` and a due-time loop to reuse), a run state
      written by the server, and the screen reading that state instead of
      remembering the last response. The transfer must then run outside the HTTP
      request, which also closes the request-shaped-transfer debt recorded in
      `docs/guides/cloud-sync.md`.

*Tests:* 39 unit, 26 integration (the suite now covers the real config file
through `RCLONE_CONFIG`, the `0600` mode and the parsed section edits over HTTP;
it still states explicitly that **no test performs a real transfer or mount**),
29 E2E. The mount-ready poll, the shutdown unmount and the `0600` repair are
covered by unit/integration tests, never by a live FUSE mount.
*Docs:* `docs/guides/cloud-sync.md` (*Known debt*).

---

### 24. Server control — the user cannot tell success from refusal

Closed on 2026-10-24 — see Done. Both findings were fixed in one pass and the
contract now lives in [guides/server-control.md](docs/guides/server-control.md)
(UI feedback) and [guides/power.md](docs/guides/power.md) (one primitive, one
sudoers grant). Tests: `handlers/system.rs` 0 → 4 unit, `power_integration.rs`
4 → 9, new `tests/e2e/desktop/power.spec.ts`.

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

### 33. Transfer quotas — residual (leftover of the closed item 23)

- [ ] **33.1 — Group membership costs one `id -nG` fork per quota check.**
      `load_applicable` (`src/transfer_quota.rs`) resolves the caller's groups
      through `GroupService::get_user_groups` on every check — and the
      resumable-upload path re-checks per chunk, so a 1 GB upload in 8 MB
      chunks forks ~128 times. It is correctly inside `spawn_blocking` (never
      on the runtime), so this is throughput debt, not a stall. *Acceptance:* a
      short-TTL (a few seconds) in-process cache keyed on the username, with
      the bounded staleness documented — the trade is a freshly added group
      quota taking up to one TTL to apply, which must be stated in
      `docs/guides/transfer-quotas.md`.

---

## Cross-cutting — fix once, unblocks many

- [ ] **X.1 — Blocking work on the async runtime.** `std::process::Command`,
      `std::fs` and `docker compose` are called straight from `async fn` in:
      Health (`web/mod.rs:322`), Dashboard (`dashboard.rs:1686,1798,1844,1994,2147,2159,2195`),
      Chat (`chat.rs:989,1021,1040`), Cache (`handlers/cache.rs:97,165,252,457`),
      Encryption (`handlers/encryption.rs:210,345`), Apps Center
      (`docker_catalog.rs:490,562`), Explorer (`explorer.rs:665,2628,2659`).
      *(Autostart left this list with item 25, 2026-10-24 — `is_enabled` /
      `set_enabled` run in `spawn_blocking`, and the two `sudo` shell-outs they
      sit next to in `handlers/settings.rs` — `hostnamectl set-hostname` and
      `timedatectl set-timezone` — followed in the same change: both moved to
      `spawn_blocking` with a 30 s `run_timed` deadline, so no
      `std::process::Command` of that module is reachable from an async body
      any more. That also closed X.2 for `settings.rs`.)*
      *(Notifications, Antivirus, Power and rclone were on this list and are
      closed — items 13 and 16, 2026-10-23; DB backup left it on 2026-10-24 with
      item 22 — every `std::fs` call of `db_backup.rs` now runs on
      `spawn_blocking`.)*
      **Reference implementations already in the tree:** `src/mdns.rs:449`,
      `src/handlers/journal.rs:171`, `src/transfer_quota.rs:136`,
      `src/handlers/docker.rs`, `src/handlers/mcp.rs:588`,
      `src/handlers/syncthing.rs::read_api_key` (2026-10-23) and
      `src/handlers/vm.rs::blocking_io` (2026-10-23).
      *Acceptance:* zero `std::process::Command` / blocking `std::fs` reachable
      from a handler's async body. A CI grep (or a clippy lint) would keep it
      that way.
- [ ] **X.2 — No timeout on subprocesses.** Absent from `disk.rs`, `pools.rs`,
      `encryption.rs`, `cache.rs`, `snapshots.rs`, `health.rs`, `network.rs`,
      `vpn.rs`, `shares.rs`, `explorer.rs` (*rclone left this list with item 16,
      2026-10-23*). Present
      in Docker, Firewall, Restic, Syncthing, disk quotas (2026-10-22), System
      Journal (2026-10-23, `JOURNAL_READ_TIMEOUT`), and — since 2026-10-23 —
      **everything the Dashboard/Widgets surface shells out to**, through the
      shared `src/utils/process.rs` (`run_timed` / `run_timed_async`,
      `HOST_PROBE_TIMEOUT` 10 s, `SMART_PROBE_TIMEOUT` 20 s) plus
      `utils/proc.rs`'s `probe()`. That module is the reference implementation
      now: a new handler should call it rather than inventing a deadline.
      A wedged `zpool`/`mdadm`/`smartctl`/`df` still blocks a handler or a job
      forever in the files listed above.
- [ ] **X.3 — Empty `else {}` error handling in the UI.** CloudSync (6),
      Health, Desktop (power), NotificationCenter — *Dashboard is fixed
      (2026-10-23: `AppUsageBreakdown` shows a `role="alert"` band for a failed
      restart/stop/uninstall and an admin-only notice instead of a silent
      403 loop); Downloader's 5
      are fixed (2026-10-23: a dismissible `Alert` with the server reason),
      Syncthing's 3 are fixed (2026-10-23: one page-level `Alert`), Explorer's
      11 sites are fixed (2026-10-22, `dlg.notify` + partial-batch messages) and
      the Journal's 5 are fixed (2026-10-23: error state + Retry, in-modal
      errors, a page-level banner)*. Direct violation of
      simplicity rule 5 ("every action gives visible feedback"). A shared
      `notifyError()` helper already exists (`Settings.svelte` uses
      `notifySettings`) — use it.
**X.4 — Documented invariants the code does not enforce.** Closed on
      2026-10-23 — see Done (2026-10-23 (r)): every invariant on the list is
      now enforced (scheduled-task overlap guard, "RAID never destroys", opt-in
      update check, single-flight cache mover, unique `(pool, prefix)`, metrics
      gaps, per-user notification read state). Its last remaining instance
      (DB-backup serialisation, tracked as item 22.1) was closed on 2026-10-24,
      so the pointer is definitively redundant.
- [ ] **X.5 — `TESTING.md` counts drift where the specs grew.** The Dashboard row
      stated "37 runtime tests, recounted on 2026-09-14 via
      `grep -cE '^\s*test\('`". *Re-counted 2026-10-23 with both figures: 48
      runtime (`npx playwright test --list --project=firefox`) / 45 `test(` (the
      grep, lower because §5 is a responsive loop).* Keep the dual figure
      whenever a responsive loop makes them differ — as the Quotas row does
      (38 runtime / 36 `test(`). The same recount applies to the rows this
      change touched.
- [ ] **X.6 — Tests verify shape, not behaviour.** None of the P0 defects is
      caught by the 50+ integration tests around the same code, because they
      assert status codes and `total`. For each item above, the "Tests" line
      names the assertion that would have caught it — add that one first.
- [ ] **X.7 — Hand-rolled auth checks that bypass `require_auth`.** The
      Dashboard's two (`dashboard.rs:2274,2328`, no `expires_at` predicate) are
      **fixed** (2026-10-23): both preference handlers now go through
      `require_auth`, so an invalid *or expired* session answers `401` and
      writes nothing. Still open: the whole `handlers/security.rs` session
      surface trusts the caller's own token as the scope (14.1/14.2). Audit
      every handler that reads `x-session-token` manually instead of going
      through `admin_guard`.
- [ ] **X.8 — A missing `x-session-token` is `400`, not `401`.** `require_auth`
      answers `json_error` (`StatusCode::BAD_REQUEST`) when the header is
      absent, and `401` only for an invalid/expired token
      (`src/handlers/admin_guard.rs:76-84`, `src/utils/response.rs:87-93`). So a
      client that branches on the status reads an auth failure as a validation
      error. Item 15 fixed the *security* half (an expired session was
      accepted) but deliberately did **not** flip the tree-wide status: the
      change is one line in `admin_guard` plus a sweep of ~50 assertions across
      ~25 integration suites (many of them loops that mix guarded endpoints with
      hand-rolled ones, e.g. `settings_core`, `chat`, `snmp`), and it cannot be
      validated without running the tests. *Acceptance:* `401` for a missing
      token in `require_auth`/`require_admin`, every `assert_eq!(… BAD_REQUEST)`
      on a no-token request flipped to `UNAUTHORIZED`, suites with mixed guards
      split per endpoint, and `docs/API.md` stating the contract once.

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

**2026-10-24 (c)** — Autostart — the `systemctl` calls left the async runtime
and a refusal now says why (items 25.1 and 25.2). `is_enabled()` /
`set_enabled()` ran `std::process::Command` straight from `async fn` handlers,
so a `sudo systemctl enable` waiting on the sudo PAM stack or a systemd job
parked a Tokio worker and — because the one Save button flushes the group —
stalled the whole settings save. Both now run in `spawn_blocking` inside
`run_timed` (`AUTOSTART_PROBE_TIMEOUT` 10 s, `AUTOSTART_UPDATE_TIMEOUT` 30 s),
with the argv built by a pure `args_for()` so the scoped
`systemctl enable|disable homevault` grant is pinned by a unit test and the
unit name stays a constant. The refusal was invisible: any `Err` became
`settings.autostart_update_failed`, discarding the stderr `set_enabled`
returned. `PUT /settings/autostart` now answers `500` whose `message` is the
host's own reason, rendered by the card as a dismissible `role="alert"`
(`settings.autostart_error`); the staged flip is reverted to the last
server-accepted value so the switch never claims a change the host refused.
`GET /settings/autostart` is tri-state — a clean non-zero exit is
`enabled:false`, a non-empty stderr (container, no systemd, unit missing) is
`500` with the reason — and a failed read disables the switch
(`settings.autostart_read_failed`) instead of showing a false "off". The
sudoers grant is unchanged. Tests: `src/autostart.rs` 0 → **9** unit,
`settings_core_integration.rs` 24 → **28** (a `PATH` shim stands in for
`sudo`/`systemctl`), `settings.spec.ts` +3.5/3.6 (mocked routes). Written,
not run (per instruction). Docs: [guides/settings.md](docs/guides/settings.md),
[API.md](docs/API.md). The two `sudo` shell-outs of the same module
(`hostnamectl set-hostname`, `timedatectl set-timezone`) left the async runtime
in the same change and gained a 30 s deadline — X.1/X.2, and no blocking
`std::process::Command` of `handlers/settings.rs` is reachable from an async
body any more.

**2026-10-24 (b)** — Server control — a refused `restart`/`shutdown` is now
visible, and every power button runs the same privileged command (items 24.1
and 24.2). The desktop power menu's two branches were `if (response.success) {
} else { }`: the modal closed before the call, so a command refused for lack of
a sudoers grant was indistinguishable from a slow restart, and a successful one
said nothing. The desktop now disables confirm/cancel for the duration (no
double submit) and renders a dismissible band: `role="status"` on acceptance
(“Restarting… the connection will drop”), `role="alert"` with the server's own
reason on refusal, the power menu staying usable. A connection that drops
before the answer is reported as “may still have been accepted”, never as a
refusal. The Settings → HTTPS card surfaces a refused restart (`role="alert"`)
because the new certificate is not live until the process restarts. Under
that, the two privilege paths were unsound: `/api/server/*` ran a bare
`shutdown -r now` with no `sudo` and no deadline while `/api/power/*` ran
`sudo systemctl reboot`; now both — and the MCP `system.reboot` tool, a third
copy — call `power::host_power_action` inside `spawn_blocking`, so the argv
(`sudo systemctl reboot|poweroff|suspend`), the 30 s `run_timed` deadline and
the refusal shape are one. The sudoers template grants exactly those three
verbs (`src/setup.rs::power_grants`, built from `PowerAction`) and no longer
the `shutdown` binary. Tests: `handlers/system.rs` 0 → **4**, `power.rs` 5 →
**6**, `setup.rs` +1, `power_integration.rs` 4 → **9**, new
`tests/e2e/desktop/power.spec.ts` (**5 `test(` / 10 runtime**, mocked routes —
never a real reboot). Docs: [guides/server-control.md](docs/guides/server-control.md),
[guides/power.md](docs/guides/power.md), [API.md](docs/API.md). Written, not run.

**2026-10-24** — Syncthing — a folder and a device can be added and removed from
HomeVault (item 18.1, the last finding of the area). Until now the screen was a
monitor: `GET /syncthing/{folders,devices}` plus three actions (pause/resume a
paired device, rescan, restart). There was no route and no control to onboard a
share or a peer, so a new phone or a new pool folder had to be configured on the
daemon directly. **Four routes** (`POST`/`DELETE` on `/syncthing/folders[/{id}]`
and `/syncthing/devices[/{id}]`) sit behind the same `require_admin` bar as the
rest of the module, and every field is validated **before** the first daemon
call — a daemon that is down must not be able to mask a validation hole. The
folder ID is `[A-Za-z0-9._-]`, ≤64 bytes; the label is trimmed and capped at
128; the path must be absolute, free of `..`, not the filesystem root and inside
`state.path_guard.allowed_roots()` (`validate_quarantine_dir`'s rule) because
the daemon would otherwise create a folder anywhere the service can write; the
sync mode is the allowlist `sendreceive|sendonly|receiveonly`, with
`receiveencrypted` refused (a home user cannot recover one from this screen);
device IDs are normalised to eight groups of seven base-32 characters, dashes
and case optional — the check digit is *not* verified, the daemon is the
authority and its refusal is relayed — and control characters are refused. The
config read that follows doubles as the conflict check and as the allowlist of
devices a folder may be shared with; the appliance's own ID is injected into a
new folder's `devices` array, and pairing or removing it is refused, so the host
cannot be unpaired from itself. Deletes re-validate the id from the URL (it is
interpolated into the daemon path) and percent-encode it. `POST` answers
`{"id"}` / `{"device_id"}`, `DELETE` answers a confirmation string, and both are
logged as events naming the admin. **On the screen**, each tab gained an *Add*
action opening a dialog that keeps the server's refusal **inside** the dialog
with the typed values intact (a validation error is fixable, a closed dialog is
not), a success banner naming what happened, and a remove button on the detail
pane whose confirmation states the consequence — the files already on disk stay.
The path field is `PathField`, not the read-only picker, because Syncthing
creates the target directory; "Share with" pre-ticks every paired device; "start
paused" is omitted (simplicity rule 2). Tests: `syncthing.rs` 18 → **34** unit
(ID charset/length, label trim, path confinement — relative, `..`, root, outside
the roots, and no-roots-refuses-all —, mode allowlist, device normalisation from
pasted forms and its refusals, folder/device dedupe and cap, and the two request
bodies against the daemon schema), `syncthing_integration.rs` 8 → **12** (the
four routes wired, `403` without an admin session, `422` for each validation
class, `503` with the daemon down), `syncthing.spec.ts` 23 → **34 runtime / 30
`test(`** (section 7 drives a **mocked** daemon — read endpoints from fixtures,
writes recorded — so no real daemon config is ever mutated: required fields,
trimmed payload and refreshed list, a refusal staying in the dialog, a malformed
device ID, cancel-then-confirm on both removals, and the actions being labelled,
keyboard-reachable buttons) — **written, not run** (per instruction). Docs:
`docs/guides/syncthing.md` (routes, validation, *Known debt*: the per-folder
`db/status?folder=` N+1 on each poll, inherent to the daemon's API),
`docs/API.md`, `CHANGELOG.md`, `TESTING.md`, RAG entry.

**2026-10-24** — DB backup — one snapshot at a time process-wide, a failed
attempt leaves its reason on the card, and retention spares the pre-migration
restore point. **22.1 (two concurrent `VACUUM INTO`s):** the guard was a
per-instance `Arc<tokio::sync::Mutex<()>>` while every HTTP handler built a
fresh `DbBackupService`, so a manual click during a scheduled run ran both.
`src/db_backup.rs` now serialises them with one module static (`RUNNING`),
shared by every handler, the scheduler loop, the startup snapshot and
`snapshot_before_migration` — which used to bypass the service entirely. A
static rather than an `AppState` field, because that would have touched the 40+
`AppState` literals under `tests/`. A same-second second snapshot is named
`homevault-…_2.db` instead of overwriting the first (the `_` suffix keeps the
reverse-name sort reading as reverse-chronological). **22.2 (`last_error` was
hardcoded `None`, and prune could eat the restore point):** the reason of the
last failed attempt lives in a process-wide slot, is served by
`GET /settings/backup/db`, cleared by the next success and by a restart, and the
Settings card renders it as a `role="alert"` line quoting the server's message;
pruning now partitions the directory — `retain` bounds the regular snapshots,
the `.pre-migration.db` restore points are never removed by it and are bounded
separately at `PRE_MIGRATION_RETAIN` = 3 newest — and they no longer consume a
`retain` slot (with `retain = 2` the old code kept one real snapshot and deleted
the other *and* could delete the restore point). `DbBackupEntry.is_pre_migration`
marks them for the screen, which labels them *Before upgrade*. **Also (X.1,
this module):** every `std::fs` call left the async runtime (`create_dir_all`,
the stale `.tmp` unlink, `rename` + `metadata`, `read_dir`, the retention
`remove_file`, and the `data/db_backup.json` load/save/delete), and
`list`/`prune`/`status`/`is_due` are `async` because they reach the filesystem;
handlers use the I/O-free `DbBackupService::new` plus `load(db).await`, which
reads the schedule on the blocking pool. Tests: `db_backup.rs` 5 → **11** unit
(the lock is held from another instance and a snapshot must wait; prune keeps
and bounds restore points; a `_2` duplicate sorts newest), `db_backup_integration.rs`
12 → **15** (planted restore point survives three snapshots at `retain = 2` and
is flagged, six restore points + a settings save keep the newest three, a
snapshot failure is a `500` with `last_error` naming the directory and the next
success clears it), `settings.spec.ts` §15 3 → **4** `test(` (§15.4 renders a
served `last_error` as an alert) — **written, not run** (per instruction).
Docs: `docs/guides/backup-restic.md` (*Only one snapshot runs at a time*,
*Retention never eats the pre-upgrade restore point*, *A failure is remembered*,
*Nothing blocks the async runtime*), `docs/API.md`, `CHANGELOG.md`, `TESTING.md`.

**2026-10-23 (r)** — Notifications — the read state is per user, the feed is
paginated, and the shared delete paths are admin-only (items 21.2, 21.3).
**21.2 (no per-user read state, despite the code claiming it):** the global
`notifications.read_at` column is gone (migration `058_notification_reads.sql`
rebuilds the table) and read state lives in the `notification_reads`
`(notification_id, username, read_at)` join table — `mark_read` /
`mark_all_read` take the caller's username, `list_recent` / `unread_count_for`
resolve that caller's `read_at` through a LEFT JOIN / NOT EXISTS, so one user
marking "all read" never clears another account's badge. `DELETE
/notifications/{id}` and the new bulk `DELETE /notifications`
(`handle_notifications_clear_all`) are now `require_admin` — the feed is
broadcast, so removing an alert-trail entry is a shared, destructive operation
(least privilege: reads `require_auth`, mutations `require_admin`). Unknown ids
on `mark_read`/`delete` answer `404` instead of a fake success.
**21.3 (no pagination; "clear all" was N unawaited DELETEs):**
`GET /notifications?limit=&offset=` answers `{ notifications, total, limit,
offset }` (limit clamped ≤200, default 50); the frontend keeps the page
window and offers a 44px "Load older" button while `offset + len < total`;
"clear all" is now **one awaited** bulk call and every mutation
(mark-read/delete/mark-all/clear) awaits the server and surfaces a failure as
a toast instead of an optimistic update that lies until the next 30s poll.
Per-item delete and "Clear all" are hidden for non-admin accounts. The list
response shape changed, so `CONTRACT_VERSION` went 1 → 2.
**X.4 closed as a side effect:** the cross-cutting "documented invariants the
code does not enforce" pointer is gone — the scheduled-task overlap guard,
"RAID never destroys", the opt-in update check, the single-flight cache mover,
`(pool, prefix)` uniqueness, the metrics gaps and the per-user notification
read state are all enforced now, and its last remaining instance (DB-backup
serialisation) is already tracked as item 22.1.
Tests: `notifications_integration.rs` 18 → **26** (per-user isolation,
caller-scoped mark-all, admin-only delete/clear-all, `404`s, pagination,
clamped limit) — **written, not run** (per instruction); `notification_engine.rs`
stays 2; `handlers/notifications.rs` 0 (no unit tests, handler logic covered by
the integration suite). `npm run check` run? — no (not run per instruction).
Docs: `docs/guides/notifications.md` (per-user read state, pagination,
admin-only delete, invariants), `docs/API.md` (endpoint table).

**2026-10-23 (q)** — Settings — the remaining cards — the four dead
settings now do what the screen says, and every host call is bounded.
**13.2 (a UPS save reloaded nothing):** the sudoers template had **no `nut-*`
grant at all**, so `restart_nut()` could never succeed; `src/setup.rs` now
writes a scoped block from `nut_grants()` (`tee /etc/nut/ups.conf`,
`systemctl restart nut-driver-enumerator.service` and
`systemctl restart nut-server.service`, plus a read-only `upsc`), with a test
asserting the grants cover the conf file and **both** units. `save_config` /
`delete_config` / `reset_config` return `Result<Vec<String>, UpsError>`: the
file is written first, then each unit that refused is collected, and the
handlers answer `reload_warnings` — a save is "written", not necessarily
"live". `upsc` reads validate the device name (`validate_ups_name`) and the
variable (`validate_ups_variable`) before the argv, every call runs on
`spawn_blocking`, and `NUT_QUERY_TIMEOUT` (10 s) / `NUT_RESTART_TIMEOUT` (60 s)
bound them. `get_ups_variable` also called `upsc <var>` with the arguments
swapped, which it no longer does.
**13.3 (SMTP fields were stored and ignored):** with a relay configured,
`email.rs` generates a private (`0600`) msmtp configuration at
`data_dir()/msmtp.conf` (`msmtp_config_text`/`write_msmtp_config`) and sends
through `msmtp --file <path>`, so host/port/user/password/TLS/From come from
the Settings card and take effect on the next notification; a blank or
`localhost` host keeps the legacy `mutt` → `sendmail` → `msmtp` order for
hosts that already deliver mail locally. stderr is drained by a reader thread
while the child runs (a chatty MTA cannot defeat the 30 s deadline) and its
tail is what the error shows; `is_email_configured()` caches its three probes
instead of shelling `which` per call. Engine sends run in `spawn_blocking`.
**13.4 (the language reverted to English on restart):** the choice is
`GeneralSettings.language`, persisted by `handle_set_language` and by the
aggregate save, and `apply_loaded_settings` calls `I18n::set_locale` during
`init_persistence` — before the server accepts its first request.
`normalize_language` keeps the primary subtag (`fr-BE` → `fr`) and refuses the
rest with a `400`; `reset_settings()` returns the process to `en`; the card
surfaces a failed save instead of looking saved.
**13.5 (dead and half-wired settings):** `scan_archives` reaches the command
line through `clamscan_args` as an explicit `--scan-archive=yes|no`;
`quarantine_dir` is validated (`validate_quarantine_dir`: absolute, no `..`,
inside `base_dir()` or a storage root) instead of being interpolated raw into
`--move=`; the SNMP `version` is read (`snmpget_args`/`snmpwalk_args`, v1/v2c
only, v3 refused with a reason) and `iface` filters the parsed table;
`reset_settings()` now also clears antivirus, anti-ransomware, per-disk
spindown, `data/db_backup.json` and `ups.conf` through
`reset_domain_settings()`, which returns per-domain `warnings` (the response
stays `success: true` and the page names what failed). `webhook_url` is
validated at save **and** dispatch (`validate_webhook_url`: http(s), a host, no
embedded credentials, ≤2048) and every alert reuses one `reqwest::Client` with
a 10 s / 5 s-connect deadline instead of `Client::new()` with none. Export
carries `schema_version`, `validate_import` names the offending field and
and refuses a newer schema.
**X.1/X.2, in scope:** every `clamscan`/`definitions`/`hdparm`/`snmpd`/
`snmpget`/`snmpwalk`/NUT call left the async runtime (`spawn_blocking` or
`spawn_blocking`-backed `*_async` helpers) and carries a `run_timed` deadline;
`antivirus_scheduler`'s due decision became the pure, tested
`is_due(cron, last_scan_at)`.
**21.1 closed as a side effect:** the notification engine's email send is now
`spawn_blocking` and its webhook reuses the timed `webhook_client()`, so the
item was removed from section 21 rather than re-opened.
Tests: `handlers/settings.rs` 17 → **25**, `settings_persistence.rs` 7 → **13**,
`ups.rs` 7 → **14**, `antivirus.rs` 8 → **11**, `snmp.rs` 5 → **9**,
`email.rs` 0 → **9**, `notification_engine.rs` 0 → **2**,
`antivirus_scheduler.rs` 0 → **4**; `settings_core_integration.rs` 19 → **24**,
`security_integration.rs` 25 → **26**, `snmp_integration.rs` 6 → **8**,
`ups_integration.rs` 4 → **5** (a bad name/driver/port is a `400` before
`sudo` is reached); `settings.spec.ts` +3 (SNMP version options + hint, UPS `reload_warnings`,
partial-reset warning) — **written, not run** (per instruction);
`cargo check --all-targets` clean, `npm run check` 0 errors.
Docs: `docs/guides/settings.md` (language, outside-JSON reset table, UPS/NUT,
SNMP, import format), `docs/guides/notifications.md` (msmtp transport, webhook
delivery), `docs/guides/antivirus.md` (argv/quarantine validation, async
probes), `docs/guides/power.md` (deadlines, reset), `docs/API.md`.
X.1/X.2 stay open as cross-cutting items for the modules not in this list.

**2026-10-23 (p)** — Cloud Sync (rclone) — every call is bounded, a mount is
owned by the process that started it, and the credential file is private.
**16.1 (no deadline on any rclone call):** `src/utils/process.rs` gained
`run_timed_async_result` with a named `ProcessFailure { Spawn { program, error },
Timeout { program, deadline } }`, so "not installed" and "too slow" no longer
collapse into one `io::Error`; `run_timed_async` delegates to it and keeps its
`.ok()` contract (5 tests). Four constants in `src/handlers/rclone.rs` replace
every bare `.output()`: `RCLONE_PROBE_TIMEOUT` 10 s (`rclone version`,
`listremotes`, `fusermount -V`), `RCLONE_MOUNT_READY_TIMEOUT` 20 s,
`RCLONE_UMOUNT_TIMEOUT` 30 s (`fusermount -u`, `umount -f`) and
`RCLONE_TRANSFER_TIMEOUT` 6 h (a ceiling, not an estimate — a first sync of a
large library legitimately takes hours). `sync`/`copy`/`jobs/{id}/run` now
complete or fail the `cloud_sync` activity job **with the reason** on every
branch, including the abort, so a stalled remote no longer holds the HTTP
request and the job row open forever.
**16.2 (no shutdown cleanup, fire-and-forget mounts):** `handle_mount_start`
dropped `--daemon`, keeps the `Child` (`kill_on_drop(false)`) in a process-wide
`static MOUNTS: LazyLock<Mutex<HashMap<String, MountedRemote>>>` keyed by mount
point (a static rather than a new `AppState` field, which would have touched the
40+ `AppState` literals in `tests/`), drains stderr into a bounded 4 KiB tail
(a full pipe would block the child) and answers success only once
`wait_until_mounted` finds the mount point in `/proc/self/mounts` — a mount that
"started" but never appeared is an error quoting rclone's last stderr lines, and
the child is killed. `handle_list_mounts` reads `/proc/mounts` with a whole-field
match and `\040`-style decoding, enriches each entry with the remote and pid from
the registry, and reports a tracked child whose mount disappeared as
`is_active: false` so the screen offers a real "Stop". `handle_mount_stop` tries
`fusermount -u`, then `umount -f`, reports **both** reasons, then kills and reaps
the child. `shutdown_mounts()` runs from `shutdown_signal` in `src/web/mod.rs`:
each mount is unmounted, then its child killed — a restart no longer leaves stale
FUSE mounts and orphaned processes.
**16.3 (credentials on disk, fire-and-forget argv, string surgery):**
`write_private_config` opens with `.mode(0o600)` and re-applies the mode, so an
older 0644 config is **repaired**; the dated backup is written by re-reading the
file (the old `tokio::fs::copy` recreated it with the source mode). Section edits
are parsed: `config_has_section` matches a whole trimmed header line and
`remove_section` ends a section at the next line *starting* with `[`, so deleting
`my` no longer hits `[mydrive]` and `patterns = [a-z]` no longer truncates a
section. `add_remote` validates the remote name (no `: [ ] / \`, whitespace,
control characters or leading `-`, ≤64) and the provider slug — `type =
{provider}` is written verbatim, so a newline there was the same INI injection as
the free-text field. `validate_mount_point` confines every mount to a strict child
of `RCLONE_MOUNT_BASE` with no `.`/`..` component, `validate_remote_subpath`
refuses an absolute or traversing path, the remote must exist in the resolved
config, and `GET /rclone/status` now returns `{ available, fusermount,
mount_base }` so the screen derives its mount point from the server's rule
instead of hardcoding `/mnt/rclone/<name>`.
**Residual defects:** `/mount/stop` required a full `MountRequest` while the UI
sends `{ mount_point }`, so every real unmount was a **422** before the handler —
fixed with `MountStopRequest`; the screen swallowed every failure — seven empty
`else {}` blocks (add/delete remote, mount, unmount, sync, save job, save
config), two job handlers that only flipped a badge and five loaders that
dropped their error — one dismissible page-level `Alert`
(`data-testid="cloudsync-error"`, `rclone.error_title` + the server's reason)
now covers them all (a failed jobs refresh keeps the list instead of blanking
it); the sync selects offer `local`-type remotes again
(`build_rclone_path` supports them — the `cloudRemotes` filter was screen-only);
`std::fs::create_dir_all` left the async runtime (`spawn_blocking`).
Tests: `handlers/rclone.rs` 15 → **39** unit (validators, `config_has_section`/
`remove_section`, `parse_mounts_for`/`decode_mount_escape`, pure
`parse_rclone_config_text`, deadline ordering, `CONFIG_FILE_MODE`,
`RcloneStatus`), `rclone_integration.rs` 17 → **26** (0600 config + repaired
0644 + 0600 backup, exact-section delete, prefix-named add, provider injection,
mount point/remote refusals with nothing created, the `{ mount_point }` body),
`cloudsync.spec.ts` 24 → **29** (3.6–3.8 guards, section 7 failure surface).
Written on 2026-10-23; **not run yet** (per instruction); `cargo check
--all-targets` clean and `npm run check` 0 errors. `docs/guides/cloud-sync.md`
rewritten (deadline table, mount lifecycle, config hardening, invariants, known
debt), `docs/API.md` gained the request/response shapes and the rejection
strings. Left open on purpose as P2 **16.4**: the scheduler.

**2026-10-23 (o)** — Dashboard / Widgets — tiles belong to an account, every host
probe has a deadline, and nothing presented as a measurement is a constant.
**15.1 (widget CRUD was global and unscoped, and an expired session still read
and wrote preferences):** migration `057_widgets_per_user.sql` rebuilds
`widgets` with `(username, id)` as the identity, `username NOT NULL` and
`ON DELETE CASCADE`, copying legacy `username IS NULL` rows to every existing
account instead of dropping them (they were one grid shared by everybody, and
no visible tile may disappear on upgrade). `WidgetRepository` lost every
unscoped method: `find_by_id`/`find_by_username`/`count_for`/`create`/
`delete_for → bool`/`delete_all_for`/`reorder_for → u64` all take the caller's
username, which comes from `require_auth` and never from the body; a tile the
caller does not own is a `404`, not a `403` (the endpoint must not confirm that
another account owns that id). `POST /dashboard/widget` validates every field
before SQL (`MAX_WIDGET_ID_LEN` 64, `MAX_WIDGET_TITLE_LEN` 120,
`MAX_WIDGET_POSITION` 64, `MAX_WIDGET_SPAN` 6, `MAX_WIDGETS_PER_USER` 64,
`size` = `NxN` with each side `1..=6`, slug charset, no control characters in
the title, title stored trimmed). `GET /dashboard/widget/{id}` answers `404`
for an unknown id instead of `200 {"type":"unknown"}`. The two
widget-preference handlers stopped hand-rolling `SELECT username FROM sessions
WHERE token = ?` — the statement that had **no `expires_at` predicate** — and go
through `require_auth`, so an expired session is `401` and writes nothing (a
test proves the table stays empty). `monitor_layout` validates the blob
structurally (JSON object, ≤ 16 KiB, depth ≤ 4, ≤ 512 nodes, keys ≤ 64 chars,
strings ≤ 256) while still storing the accepted bytes **verbatim** — a
`serde_json::Value` round trip would reorder keys and break the byte-for-byte
read the frontend relies on. **15.2 (untimed subprocess fan-out every 5 s):**
new `src/utils/process.rs` (`run_timed`, `run_timed_async`,
`HOST_PROBE_TIMEOUT` 10 s, `SMART_PROBE_TIMEOUT` 20 s) is now the single
deadline helper — `utils/proc.rs` wraps all 20 of its probe sites in one private
`probe()`, `dashboard.rs` wraps its 15 (plus the four `smartctl` calls on the
wider deadline), and the journal's private copy of `run_timed` was deleted in
favour of a delegation. `/dashboard/health` serves a 30 s `DISK_HEALTH_TTL`
snapshot coalesced by a mutex, so the 5 s poll no longer re-forks `lsblk` + `df`
+ 1-3 SMART commands per disk; the browser keeps its 5 s interval (E2E 3.1
pins it) but skips ticks while `document.hidden` or while a request is in
flight, and refreshes immediately on `visibilitychange`. **15.3 (fabricated
values):** the disk score is SMART 40 / temperature 20 / usage 40 — the
hardcoded `error_score = 10` is gone, so a failed disk scores 0 instead of 10
and a perfect one reaches 100; the status vocabulary is `healthy | warning |
unhealthy` (`health_status_for_score`, the single definition — the fifth word
`critical` nothing rendered is gone; `DiskSummary.critical_count` stays, it is a
count); CPU % is a per-caller `/proc/stat` delta (shared sampler, bounded at
256 keys) instead of `load1 / cores`, and the same sampler feeds the health
endpoint, `/dashboard/widget/cpu` and `/widgets/cpu`, so the Dashboard and
Metrics cannot disagree; `/dashboard/widget/memory` returns real
`/proc/meminfo` numbers; `has_server_update` was deleted from
`UpdateInfoResponse` (constant `false` in both branches, no consumer);
`PREV_NETWORK_STATS` is keyed per account with a first-sample rate of `0` and
saturating subtraction; `CpuDetails.usage_percent` was added (mean of
`per_core_usage`) because the app-usage card read a field that did not exist;
`Dashboard.svelte` dropped the static `UP TO DATE` badge and the literal
"last test 2", derives `healthHint` from the real status, computes network rates
from the actual elapsed time and reads `iface.link_speed_mbps` (it read
`iface.speed`, always `undefined`); the Docker card is gated on `$isAdmin`
(admin-only endpoint polled by every viewer before), skips hidden ticks and
renders a failed action as a `role="alert"` band; `monitorStore` gained
`savedAt` (last write wins, so a login on a second origin can no longer revert
a fresh layout); the dead `app-update`/`server-update` routes were removed from
`widgetData.ts`. Tests: `dashboard.rs` 23 → **40** unit (`monitor_layout.rs`
0 → **13**, `proc.rs` 0 → **7** for the new CPU samplers, `utils/process.rs`
**5**), `dashboard_integration.rs` 10 → **19** (per-account scoping, a
foreign/unknown id is `404`, an expired session is `401` and writes nothing,
the field-validation matrix, the 64-tile cap, real memory numbers),
`widgets_integration.rs` 9 → **14** (object/size/depth/key/string rejection, a
16 KiB boundary blob round-trips, `has_server_update` absent), `dashboard.spec.ts`
40 → **45** `test(` (health hint follows the status, real SMART footer, no
`UP TO DATE` badge, hidden-tab polling, admin-only usage card) and
`widgets.spec.ts` 20 → **22** (the `savedAt` stamp, a malformed blob refused and
the stored layout untouched). Written on 2026-10-23; **not run yet** (per
instruction). `docs/guides/dashboard.md` gained *Tiles are per-account*,
*Host probes run under a deadline* and *Nothing shown as a measurement is a
constant*; `docs/guides/widgets.md` gained *`savedAt`* and *the blob is
validated before it is stored*; `docs/API.md` documents the `404`s, the
per-account scoping, the layout bounds and the removed `has_server_update`.
The one thing deliberately **not** done is the tree-wide `400 → 401` for a
*missing* `x-session-token`: it is one line in `admin_guard` plus a ~50-assertion
sweep across ~25 suites that cannot be validated without running them, so it is
recorded as **X.8** with the exact scope.

**2026-10-23 (n)** — Metrics — a failed read is a gap instead of a `0.0`, and a
rate is divided by the real elapsed time. **11.1 (a failed `/proc` read was
stored as `0.0`, and the schema forbade gaps):** migration
`056_metrics_nullable_series.sql` rebuilds `metric_samples` with `cpu`, `mem`,
`load1`, `net_rx` and `net_tx` nullable (SQLite has no `ALTER COLUMN`, so the
table is rebuilt, the existing rows copied verbatim and the index recreated);
`sample_from` returns `None` for a series whose file could not be read, and
`store_sample` binds it as `NULL`. `RawSample` is now a `FromRow` struct of
`Option<f64>`s, and `bucket_samples` keeps **one accumulator per series**, so a
bucket where CPU came back twice and the thermal zone once reports the mean of
the two CPU readings rather than a CPU figure halved by the missing sensor.
**11.2 (rates divided by a constant 60 s):** the collector keeps
`PrevSample { at, counters }` and divides the network delta by the real
wall-clock distance between the two reads; the loop sets
`MissedTickBehavior::Skip`, so a suspend can no longer fire the missed ticks
back-to-back into one minute bucket where `INSERT OR REPLACE` kept only the
last. A first sample after a boot, a counter reset and a frozen clock store no
rate at all instead of a zero, and the three redundant `/proc` reads per tick
became one `read_counters()` that reads each file exactly once. **Also:** the
five `/proc`/thermal parsers are pure (`parse_proc_stat`, `parse_meminfo`,
`parse_loadavg`, `parse_net_dev`, `parse_thermal_temp`) and unit-tested on
fixtures; `GET /metrics/latest` answers `200` + `data: null` on a fresh install
instead of `400 "No metrics collected yet"`. **Frontend:** the empty state is
decided on all six series (a temperature-only dataset rendered "no data"), every
`load()` takes a ticket so a slow `24h` answer cannot repaint a newer `1h`, and
a missing figure prints `—` instead of `0.0`. Tests: `metrics.rs` 5 → **19**
(per-series averaging, all-null bucket, `sample_from` matrix, parser fixtures),
`metrics_integration.rs` 5 → **8** (a stored `NULL` is a gap while a stored
`0.0` stays a zero, a null series does not drag its neighbours, `latest` keeps
a null series null, the empty-DB `latest` is `200`), `metrics.spec.ts` 4 → **6**
`test(` (§5.1 temperature-only dataset, §5.2 out-of-order range answer).
Written on 2026-10-23; **not run yet** (per instruction).
`docs/guides/metrics.md` gained *Null semantics* and *Rates divide by the real
elapsed time*; `docs/API.md` documents the nullable series and the `data: null`
answer.

**2026-10-23 (m)** — Transfer quotas — the guard now fails closed, the counters
count a real calendar window, and reads are scoped in the handler instead of the
screen. **23.1 (a DB error disabled every cap):** all six File Explorer quota
loads (download, download-ticket, the legacy multipart `/upload`, resumable
`/upload/session`, `/upload/chunk`, `/upload/complete`) go through one
`load_quotas_or_refuse`
(`src/handlers/explorer.rs`); a load error answers **`503`** with
`transfer_quota::unavailable_message(...)` instead of being swallowed into an
empty quota list — "the database could not tell me" is no longer read as "this
subject has no quota". A source-level unit test
(`test_no_quota_load_bypasses_the_fail_closed_gate`) pins the invariant that the
only two calls to `applicable_*_quotas` in that file are inside the helper.
**23.2 (lifetime counters):** migration
`055_transfer_quota_period.sql` adds `period` (`lifetime` default — no behaviour
change for an existing row — `day`, `week`, `month`) and `period_started_at`.
Windows are **calendar** windows in the appliance timezone
(`transfer_quota::appliance_timezone()`, Settings → General): midnight, Monday
00:00, the 1st at 00:00 — so "100 GB/month" resets on the 1st, not 30 days after
the first upload, and a DST shift keeps the local midnight
(`local_midnight` probes for a zone whose 00:00 does not exist instead of
panicking). Two paths close a window and agree by construction because both go
through `window_containing`: the **enforcement path** settles a stale row the
moment any transfer consults it (`settle_window` → `reset_window`; the common
case writes nothing), and a **reset loop** (`run_reset_loop`, 60 s,
`MissedTickBehavior::Skip`, spawned in `src/web/mod.rs`) keeps the screens
honest — so a stopped scheduler can delay the *display* of a reset, never the
reset itself. Changing a row's `period` zeroes both counters (a `CASE` in
`set_limits`), because billing lifetime history against a newly chosen monthly
cap would instantly block the subject; editing only a limit keeps the counters.
`period_resets_at` is computed for the wire and never stored.
**23.3 (any user could read any subject's usage):** the list endpoint is now
`require_admin` and the single-subject `GET` is **self-or-admin**
(`subject_type = user` **and** caller = `subject_name`, else admin) — which also
closes **30.1**, whose client-side-only gate was the same root cause. **Also:**
`bytes as i64` is replaced by a saturating `bytes_to_i64` and `check_quota`
compares in `u128` (a `u64` above `i64::MAX` used to wrap negative and pass the
check), usage counters are floored with `MAX(0, …)` so a refund landing after a
window reset cannot go negative, `QUOTA_LOCKS` prunes the mutexes no transfer
holds (`Arc::strong_count > 1`) instead of growing for the life of the process,
and `set_limits` takes one `QuotaLimits` value rather than seven positional
arguments. **Frontend:** a *Counting period* select in the Users and Groups
quota cards (`users-quota-period` / `groups-quota-period`) with a "Resets on …"
line, and the period + reset date on every Quotas row (`quotas-period`); 8 new
`[quotas]` keys in all four locales. Tests: `transfer_quota.rs` 9 → **25**
(period parsing matrix, day/week/month windows, December rollover, Paris vs UTC,
DST boundary, elapsed-window matrix, reset instant, saturation), the repo
`0` → **9** (period round-trip, period change zeroes and a limit edit does not,
`reset_window`, `reset_expired` scope, floor at zero), `handlers/explorer.rs`
**+1** source guard, `tests/quotas_integration.rs` **recreated** (deleted
2026-09-09) with **15** tests (auth scoping, unknown period refused, period
round-trip, counters on a period change, an elapsed window settled by the guard
with no scheduler, the reset pass, delete → default), `quotas.spec.ts` 35 → **40**
(`test(`: the period/reset line, the API-supplied instants, a lifetime row
without a reset, a member refused the list and another user's quota, a member
reading their own). Written on 2026-10-23; **not run yet** (per instruction).
`docs/guides/transfer-quotas.md` gained *Counting periods* and *Fail closed*,
plus a *Known debt* section for the residual `id -nG` fork (now **33.1**);
`docs/API.md` documents the new auth columns, the `period` field and the `503`.

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
