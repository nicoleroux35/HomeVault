# Dashboard & Widgets

**Purpose:** System overview (CPU, RAM, disks, network, sensors, motherboard) and
the user-customizable widget system.

## Backend

Handlers in `src/handlers/dashboard.rs` and `src/handlers/widgets.rs`.
Widget persistence via `src/storage/widget_repo.rs` and
`user_widget_preferences` (migration `013_fix_user_widget_preferences_fk.sql`).

### Tiles are per-account (`057_widgets_per_user.sql`, ROADMAP 15.1)

A dashboard tile belongs to exactly one account: `widgets` is keyed by
`(username, id)`, `username` is `NOT NULL`, and the row cascades on account
deletion. `WidgetRepository` therefore has **no unscoped accessor at all** —
every read and every mutation takes the caller's username, which the handler
gets from `require_auth` (never from the request body):

| Method | Meaning |
|--------|---------|
| `find_by_id(username, id)` / `find_by_username(username)` | one / all of the caller's tiles, in display order |
| `count_for(username)` | how many the caller owns (grid cap) |
| `create(&widget)` | insert for `widget.username` |
| `delete_for(username, id) → bool` | `false` means "not yours" → the handler answers 404 |
| `delete_all_for(username)` | account teardown |
| `reorder_for(username, &ids) → u64` | only rows owned by that username; returns how many matched |

The migration copies legacy `username IS NULL` rows to **every** existing
account (they were one grid shared by everybody) instead of dropping them, so
no visible tile disappears on upgrade. Before it, `handle_add_widget` hardcoded
`username: None`, so no tile had an owner and any authenticated user could
delete or reorder every widget on the appliance.

`POST /dashboard/widget` validates every field before it reaches SQL
(`MAX_WIDGET_ID_LEN` 64, `MAX_WIDGET_TITLE_LEN` 120, `MAX_WIDGET_POSITION` 64,
`MAX_WIDGET_SPAN` 6, `MAX_WIDGETS_PER_USER` 64; `size` must be `NxN` with each
side `1..=6`; ids/types are limited to `A-Za-z0-9-_.:`; titles are trimmed and
cannot contain control characters). A tile the caller does not own answers
**404**, never 403: the endpoint must not confirm that another account has a
tile with that id.

`GET /dashboard/widget/{id}` answers `cpu` / `memory` / `uptime` with real
`/proc` values and **404 for anything else** — it used to return
`{"type":"unknown"}` with `200`, which rendered as an empty tile.

`GET|POST /dashboard/widget-preferences` now goes through `require_auth` like
every other protected handler. It previously hand-rolled
`SELECT username FROM sessions WHERE token = ?` with **no `expires_at`
predicate**, so an expired session kept reading and writing preferences while
every other endpoint refused it; an invalid token also answered `400` instead
of `401`. `user_widget_preferences.user_id` stores the **username** (see
migration `013`), which is what the guard now resolves.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /dashboard/data` | auth | CPU/RAM/disk/network summary + the caller's tiles |
| `GET /dashboard/widget/{id}` | auth | one tile's data; 404 for an unknown id |
| `POST /dashboard/widget` | auth | add a tile to the caller's grid (validated) |
| `DELETE /dashboard/widget/{id}` | auth | remove one of the caller's tiles; 404 if not theirs |
| `POST /dashboard/widgets/reorder` | auth | reorder the caller's grid |
| `GET /dashboard/widgets` | auth | the caller's tiles |
| `GET /dashboard/network-traffic`, `/cpu-temperature`, `/storage-volumes`, `/disk-io`, `/storage`, `/health`, `/server-details` | auth | sub-metrics (subprocess probes under a deadline, see below) |
| `GET/POST /dashboard/widget-preferences` | auth | per-user panel prefs (expired session → 401) |
| `GET /widgets/cpu\|ram\|network-traffic\|cpu-temp\|update-info` | auth | desktop widgets |
| `GET /health` | auth | `HealthService::run_all_checks` + `/proc` CPU/mem |

`GET /dashboard/widget/{id}` dispatches through one live, routed function,
`handle_widget_data` (`src/handlers/dashboard.rs`), keyed by widget type —
there is no separate per-widget-type handler.

## Frontend

- `frontend/src/routes/Dashboard.svelte`.
- `frontend/src/lib/stores/dashboard.ts`, `monitorStore.ts`, `widgetData.ts`.
- Charts: the CPU per-core panel renders `charts/BarsChart` (LayerChart,
  themed with `--hv-*` tokens) — the reusable chart layer lives in
  `frontend/src/lib/components/charts/` (`DonutChart`, `BarsChart`,
  `TrendLineChart`, `AreaChart`).
- Widget components: `frontend/src/lib/components/dashboard/*`. (The
  `Widget*.svelte` / `WidgetPanel.svelte` / `desktopWidgets.ts` files were
  removed 2026-07-18 — the desktop monitor tiles are rendered inline in
  `Desktop.svelte`, see [Widgets](./widgets.md).)

### Live usage by app — `components/dashboard/AppUsageBreakdown.svelte`

A full-width card under the four stat tiles, added 2026-10-16. It polls
`GET /docker/containers?stats=true` (per-container CPU %, memory, net/block
IO — see [Docker](./docker.md)) alongside `GET /dashboard/server-details`
(host CPU/memory totals) every 5 s and renders:

- **Two stacked area charts** — CPU and memory — whose bands are the running
  apps, so the *composition* of the host total is visible over a rolling
  24-sample (~2 min) window. Drawn by `charts/AreaChart.svelte`, a hand-rolled
  SVG component (the shared `LineChart` cannot stack).
- **A per-app list** with a colour chip, live CPU/memory, and a `…` menu of
  direct actions: **restart**, **stop**, **uninstall** (force-remove, behind a
  `ConfirmDialog`).

Two invariants:

1. **Stable per-app colour.** `lib/utils/appColor.ts` hashes the container
   name to a fixed hue (djb2 → `hsl(h 62% 52%)`). It is deliberately *not* a
   per-render index and *not* an icon colour extracted at runtime (the offline
   equivalent of a colorthief read): a colour that changed every poll would
   make the stacked bands and the chips meaningless. The fixed
   saturation/lightness keep every chip above the 3:1 non-text contrast bar.
2. **The list freezes while a menu is open** (and while an action is in
   flight): `refresh()` returns early when `menuFor !== null || busyFor !== null`,
   so rows never reorder or disappear under the user's cursor mid-click. Same
   reason the Docker app freezes its list during a menu. E2E 3.4 asserts the
   row order is unchanged after a full poll interval with the menu open.

The card degrades cleanly on a host without a Docker daemon: it renders
`[data-testid="dashboard-usage-empty"]` instead of the charts/list.

**Admin-only.** `GET /docker/containers?stats=true` is `require_admin`, so a
regular user used to see a card that quietly failed every 5 s. It now renders
`[data-testid="dashboard-usage-admin-only"]` and issues **no request at all**
(the poll returns early on `!$isAdmin`). E2E 4.6 pins both halves.

**Failures are visible.** A failed restart/stop/uninstall sets `actionError`,
rendered in the same `role="alert"` band as a load failure — previously it went
to the console only.

### Host probes run under a deadline (ROADMAP 15.2)

Every command the dashboard (and the widgets/monitor code) shells out to — `df`,
`lsblk`, `mdadm`, `zpool`, `smartctl`, `ip`, `nvidia-smi`, `rocm-smi`,
`dmidecode`, `lspci`, `sensors`, `upsc`, `hostname`, `dpkg-query`/`rpm`/
`pacman` — is wrapped in a deadline from `src/utils/process.rs`:

| Helper | Used by | Deadline |
|--------|---------|----------|
| `run_timed(&mut Command, deadline)` | blocking probes | `HOST_PROBE_TIMEOUT` (10 s) |
| `run_timed_async(&mut tokio::process::Command, deadline)` | async probes | `HOST_PROBE_TIMEOUT` (10 s) |
| `SMART_PROBE_TIMEOUT` | `smartctl` only (a drive may have to spin up) | 20 s |

The child is **killed** at the deadline and the call reports an error, which
every caller already treats as "field unknown". Before, a wedge (a stopped NFS
mount, a NIC that stopped answering, a disk spinning up) held the request
forever and the 5 s poll piled requests up behind it. `src/utils/proc.rs` uses
a single private `probe()` helper for the same reason, and the journal's private
`run_timed` now delegates to the shared module instead of keeping a copy.

**The 5 s health poll does not re-probe the hardware.** `/dashboard/health`
serves a snapshot that is at most `DISK_HEALTH_TTL` (30 s) old
(`DISK_HEALTH_SNAPSHOT`, coalesced by a mutex so concurrent callers share one
collection). Without it every 5 s tick re-ran `lsblk` + `df` + up to three
SMART commands per disk. The browser keeps its 5 s interval (E2E 3.1 asserts
it) but **skips ticks while `document.hidden` or while a request is in flight**,
and refreshes immediately on `visibilitychange` (E2E 3.6).

### Nothing shown as a measurement is a constant (ROADMAP 15.3)

- **Disk score = SMART 40 + temperature 20 + usage 40.** The old score added a
  hardcoded `error_score = 10` that no disk ever earned: it lifted a failing
  disk off the floor and made a perfect one cap at 90. An unknown SMART verdict
  keeps half credit (20/40) and an unknown temperature keeps 10/20 — a probe
  that could not answer is not evidence of a fault.
- **One status vocabulary: `healthy | warning | unhealthy`** (the UI also knows
  `unknown`). `health_status_for_score()` is the single definition; the
  filesystem/volume branches emitted a fifth word, `critical`, that nothing
  rendered. `DiskSummary.critical_count` stays — that is a *count*, not a
  status.
- **CPU % is a real `/proc/stat` delta**, sampled per caller key
  (`utils::proc::cpu_usage_percent_since_last_sample`), not `load1 / cores` —
  a load figure is not a utilisation and can exceed 100 %. The same sampler
  feeds `/dashboard/health`, `/dashboard/widget/cpu` and the widgets endpoint,
  so the Dashboard and the Metrics app can no longer show two contradicting
  numbers for the same box. The first sample falls back to the since-boot
  average rather than a fabricated `0 %`, and the per-key map is bounded
  (`MAX_SAMPLE_KEYS` 256).
- **`/dashboard/widget/memory` reports real `/proc/meminfo` numbers**
  (`total`/`used`/`available` + a percentage derived from them).
- **`has_server_update` is gone** from `UpdateInfoResponse`. It was the constant
  `false` in both branches — a measurement never taken — and no client read it.
  `server_current_version` stays (it is read from the running system).
- **The OS card no longer prints an "UP TO DATE" badge.** Nothing on the box
  checks for OS updates.
- **The SMART footer prints what the drives reported** (`last_self_test`), or
  "no self-test recorded" — it used to print the literal `2`.
- **Network rates use the real elapsed time** between two samples instead of
  assuming a 5 s interval (a hidden tab or a slow response stretches it).
  `iface.link_speed_mbps` is the field the payload actually carries
  (`iface.speed` read `undefined` and always rendered `—`).
- **`CpuDetails.usage_percent`** was added (the mean of `per_core_usage`) —
  the app-usage card read a field that did not exist and fell back to `0`.
- Dead routes were removed from `frontend/src/lib/components/desktop/widgetData.ts`:
  the `app-update` / `server-update` widget ids were deleted from the catalog
  on 2026-07-18, so those two cases could only be reached by an id no tile uses.

## Invariants

- Every dashboard endpoint requires `require_auth` (a past audit found them wide
  open — no check at all). **Every** one, including the widget-preference
  handlers, which used to hand-roll a session lookup that never checked
  `expires_at`.
- A widget mutation only ever touches the caller's own rows, and a tile that is
  not theirs answers **404** (not 403): the endpoint must not confirm that
  another account owns that id.
- The "updated at" timestamp uses the active UI locale (a past bug hardcoded
  `fr-FR`).
- **Editable since 2026-09-12** — `CLAUDE.md`'s frozen-area registry is gone, so
  `Dashboard.svelte`, `src/handlers/dashboard.rs` and `src/handlers/widgets.rs`
  are open to edits. The invariants above stay binding: a change keeps the Rust
  integration + Playwright E2E suites green.

## Related

- [Storage](./storage.md), [Health](./health.md), [API Contract](../API.md)
