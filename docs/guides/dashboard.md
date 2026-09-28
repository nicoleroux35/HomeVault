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
  `TrendLineChart`).
- Widget components: `frontend/src/lib/components/dashboard/*`. (The
  `Widget*.svelte` / `WidgetPanel.svelte` / `desktopWidgets.ts` files were
  removed 2026-07-18 — the desktop monitor tiles are rendered inline in
  `Desktop.svelte`, see [Widgets](./widgets.md).)

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
