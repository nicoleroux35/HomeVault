# Dashboard & Widgets

**Purpose:** System overview (CPU, RAM, disks, network, sensors, motherboard) and
the user-customizable widget system.

## Backend

Handlers in `src/handlers/dashboard.rs` and `src/handlers/widgets.rs`.
Widget persistence via `src/storage/widget_repo.rs` and
`user_widget_preferences` (migration `013_fix_user_widget_preferences_fk.sql`).

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /dashboard/data` | auth | CPU/RAM/disk/network summary |
| `GET /dashboard/widget/{id}` | auth | one widget's data |
| `POST /dashboard/widget` | auth | add widget |
| `DELETE /dashboard/widget/{id}` | auth | remove |
| `POST /dashboard/widgets/reorder` | auth | reorder |
| `GET /dashboard/widgets` | auth | all |
| `GET /dashboard/network-traffic`, `/cpu-temperature`, `/storage-volumes`, `/disk-io`, `/storage`, `/health`, `/server-details` | auth | sub-metrics |
| `GET/POST /dashboard/widget-preferences` | auth | per-user prefs |
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

## Invariants

- Every dashboard endpoint now requires `require_auth` (a past audit found them
  wide open — no check at all). The widget-preference endpoints already validated
  sessions and are left untouched.
- The "updated at" timestamp uses the active UI locale (a past bug hardcoded
  `fr-FR`).
- **Editable since 2026-09-12** — `CLAUDE.md`'s frozen-area registry is gone, so
  `Dashboard.svelte`, `src/handlers/dashboard.rs` and `src/handlers/widgets.rs`
  are open to edits. The invariants above stay binding: a change keeps the Rust
  integration + Playwright E2E suites green.

## Related

- [Storage](./storage.md), [Health](./health.md), [API Contract](../API.md)
