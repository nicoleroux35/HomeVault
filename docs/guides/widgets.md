# Widgets (desktop monitor catalog)

**Purpose:** A "store" app for toggling the floating desktop monitor panels
on/off. Pure frontend feature — no dedicated backend surface of its own
(monitor *position* sync uses the small, unrelated `monitor-layout` endpoint
below, shared with the rest of the desktop shell).

## The 11 monitors

Each is a large, single-instance, draggable panel rendered directly in
`Desktop.svelte` (not a separate `DesktopWidget.svelte` component — an
earlier multi-instance small-widget catalog of that shape, plus 6 small
widgets — CPU/RAM/Traffic/CPU-Temp/App-Update/Server-Update — was removed
entirely on 2026-07-18 at the user's explicit request; see `CLAUDE.md`'s
Widgets app section for that history):

Resource, RAID, Security, VPN, Downloader, Syncthing, Calendar, Docker,
Weather, Active Sessions, System Health.

Each is table-driven across three files, so adding a 12th monitor means one
entry in each, not a copy-pasted block:

- `frontend/src/lib/stores/monitorStore.ts` — one `<name>Monitor: {x, y}` +
  one `<name>MonitorVisible: boolean` field, persisted per-account (see
  Persistence below), plus a `set<Name>Visible(visible)` setter.
- `frontend/src/routes/Desktop.svelte` — one `MonitorKind` case in every
  keyed table/switch (`MONITOR_WIDTH`, `MONITOR_SELECTOR`,
  `MONITOR_CLOSE_SELECTOR`, `getMonitorPosition`/`updateMonitorPosition`,
  a `<name>Clamped` reactive value — see [Desktop](./desktop.md) for why
  positions are re-clamped against the live viewport rather than read
  directly from the store), the panel's own data-loading function and
  polling interval, and the `.hv-<name>-monitor` markup + CSS.
- `frontend/src/routes/Widgets.svelte` — one `widgetDefs` entry (icon,
  color, `nameKey`/`descKey`, `visible`, `toggle`) driving the catalog grid;
  `resetAll()` also needs the new monitor added.

Weather is the one monitor with its own extra persisted state
(`frontend/src/lib/stores/weatherLocation.ts`, the saved city — deliberately
not folded into `monitorStore.ts`, which only ever holds
position/visibility) and calls the Open-Meteo API directly from the
browser rather than through a backend proxy, since a weather lookup is
neither privileged nor host-specific (unlike every other monitor here).
System Health is a pure new consumer of the already-existing, already-
admin-audited `GET /health` endpoint ([Health](./health.md)) — it needed no
backend change at all. Active Sessions needed two small backend additions:
`SessionRepository::count_distinct_active_users()` and
`GET /security/active-sessions-count` (`require_admin` — it reveals
aggregate information about *other* users' presence on the system).

## Persistence

Position/visibility live in `localStorage`, namespaced per account
(`hv-monitor-positions:<username>` — without this, two accounts sharing a
browser would silently overwrite each other's layout). They're also synced
to the backend (`GET/POST /monitor-layout`, `auth`,
`src/handlers/monitor_layout.rs`) so the same account converges on one
layout across different origins/ports; a failed sync is non-fatal, since
`localStorage` already has the latest value locally.

**Every monitor is off by default (2026-10-19).** `defaultPositions()` in
`monitorStore.ts` seeds all eleven `*Visible` flags to `false`, so a fresh
install opens on a clean desktop and the operator picks the widgets they want
from the Widgets app or Settings → Widgets. Positions are still seeded, so a
widget switched on lands in its intended slot. Only the *default* changed:
an account with a saved layout keeps its own flags (`loadFromStorage` reads
`parsed.xVisible ?? d.xVisible`), and **Reset to Default** in the Widgets app
now restores that shipped default — everything off — instead of turning
everything on.

### `refreshMs` — the widget poll interval lives in the same blob (2026-09-14)

`MonitorPositions` also carries `refreshMs: number`, the interval every
resource monitor polls at (`REFRESH_INTERVAL_OPTIONS` in
`monitorStore.ts` = 2 s / 5 s / 10 s / 30 s / 60 s; default `5000`). It is
set from **Settings → Widgets** ("Refresh interval") and read by
`Desktop.svelte`'s widget subscriptions, which are keyed
`` `${widgetId}:${intervalMs}` `` in `widgetData.ts` — a changed interval
tears the old poller down and starts a new one, so no poller leaks.

Two things to know before touching it:

- **No migration, no schema.** `POST /api/monitor-layout` stores the client's
  `layout_json` string verbatim (upsert on `username`), so the new key needs
  no backend change — `tests/widgets_integration.rs` proves both that a legacy
  blob without `refresh_ms` round-trips byte-for-byte and that a new value
  persists alongside its sibling keys. `syncFromServer` spreads
  `{ ...d, ...parsed }`, so an old blob simply keeps the default.
- **`loadFromStorage` whitelists keys explicitly** (it does not spread). A new
  field added to `MonitorPositions` is *silently dropped* on reload unless it
  is also added to that remap. `normalizeRefreshMs()` coerces anything
  unusable (absent / 0 / NaN) back to `DEFAULT_REFRESH_MS`.

The long-cadence pollers (Docker catalog, notifications) deliberately keep
their own fixed intervals — they are not "widget refresh" and are not driven
by `refreshMs`.

### `savedAt` — last write wins across origins (ROADMAP 15.3)

`MonitorPositions` also carries `savedAt: number` (epoch ms; **`0` means
"never edited"**). Every mutator stamps it (`edit()` in `monitorStore.ts`
wraps each setter), `reset()` stamps it too, and `syncFromServer` adopts the
server blob **only when its `savedAt` is at least as new** as the local one —
otherwise it keeps the local layout and pushes it. Without the stamp, logging
in on a second origin silently reverted a layout the user had just changed.
A blob saved before the field existed normalises to `0`, so the server copy
still wins exactly once.

### The layout blob is validated before it is stored (ROADMAP 15.1)

`POST /monitor-layout` stores `layout_json` **verbatim** (never re-serialised —
`tests/widgets_integration.rs` requires a byte-for-byte round trip, including a
legacy blob with no `refreshMs`), but only after a structural check: JSON
**object**, ≤ 16 KiB, ≤ 4 levels deep, ≤ 512 nodes, keys ≤ 64 characters,
string values ≤ 256 characters. A rejected blob is `400` and leaves the stored
layout untouched, so a hand-edited localStorage mirror cannot bloat the row or
break the reader.

### Edit Widgets — drag & drop layout editor (2026-10-16)

The `edit-widgets` app (`frontend/src/routes/EditWidgets.svelte`, opened from
the Applications launcher) is the layout editor the catalog app never had. It
has two zones:

- **Featured** — up to `MAX_FEATURED` (3) widgets pinned to the top of the
desktop's widget column. The cap is enforced in
  `stores/widgetLayout.ts::toggleFeatured`, which returns `false` at the cap
  (the page shows `edit_widgets.cap_reached`).
- **All widgets** — the full catalog in the account's saved order, each with a
  visibility toggle (visibility itself is still owned by `monitorStore`).

Layout (`featured` + `order`) is per-account in localStorage
(`hv-widget-layout:<username>`), namespaced exactly like `monitorStore` and
`shortcuts`. `sanitize()` appends any catalog entry a stored order predates,
so a widget added in a later release still appears.

**Drag & drop is not the only path.** Native HTML5 drag (`draggable` +
`dataTransfer`) reorders and features, but every tile also carries move
up/down and a star toggle, so the whole interaction is reachable with mouse,
touch, pen and keyboard (WCAG 2.2). E2E `edit-widgets.spec.ts` 3.1/3.2 drive
the keyboard path.

## Frontend

- `frontend/src/routes/Widgets.svelte` — the app itself, reachable from the
  Applications launcher as the `widgets` entry in `Desktop.svelte`'s `apps`
  array.
- `frontend/src/lib/stores/monitorStore.ts`, `weatherLocation.ts`.
- `frontend/src/lib/services/api.ts`: `getMonitorLayout`, `setMonitorLayout`,
  `getActiveSessionsCount`.

## Invariants

- **Shared panel geometry** — the 11 panels share one visual vocabulary from
  `tokens.css`: corner `--hv-radius-card` (18px, a floating-card corner — not
  the 12px control radius), header `--hv-widget-header-h` (44px, the app-header
  bar: a 28px accent icon tile (a **perfect circle** — `--hv-winhdr-icon-radius:
  50%`, same shape as the dock tiles / topbar buttons / app header) + 14px
  semibold title, **no close/action
  button** — monitor visibility is managed from Settings > Widgets), and the
  width stops `--hv-monitor-w-compact` (280px: Resources/VPN/Downloader/
  Syncthing), `--hv-monitor-w` (300px: Security/Calendar/Docker/Weather/
  Sessions/Health), `--hv-monitor-w-wide` (320px: RAID). Keep the three-tier
  width set — widening any compact panel by hand breaks the shared stops.

  The header's icon tile follows the **same contract as the in-window
  `WindowHeader`** (2026-10-18), because the two bars are the same object seen
  from two places: 28px tile (= `--hv-winctrl-size`), `border-radius: 50%`,
  white glyph **clamped to 16px by `.hv-widget-tile :global(svg)`** — so a
  monitor's icon snippet must **not** pass `size` (a stylesheet beats the SVG
  attribute; the 11 snippets carried `size={16}` until 2026-10-18 and had it
  removed) while `stroke` stays explicit at **1.8**, the app-header weight, so
  the two families read at one weight. The one deliberate difference is the
  tile *material*: apps are gradients (`--hv-app-<id>-grad`, the
  one-colour-per-app rule), monitors are **flat accents** from
  `MONITOR_COLOR` in `Desktop.svelte` — a monitor is not an app, so it has no
  gradient token and must not borrow one. E2E `widgets.spec.ts` `2.2` pins the
  bar height, the 28×28 tile and its `50%` radius.

  The
  geometry is locked by `tests/e2e/widgets/widgets.spec.ts` test 2.2.
- There is no second place to keep monitors in sync — see the three-file
  list above. Missing one leaves either a dead store field, a panel that
  never renders, or a catalog tile that toggles nothing.
- `refreshMs` (and `savedAt`) are persisted in the same `monitor-layout` blob
  as the positions (no migration) and must be added to `loadFromStorage`'s
  explicit whitelist, not just to the type — see the Persistence section.
  `savedAt: 0` must stay the default: it means "never edited", which is what
  lets the server copy win once for a legacy blob.
- The layout blob has no schema, so the backend's structural validation
  (object / 16 KiB / depth 4 / 512 nodes / key 64 / string 256) is the only
  defence against a malformed client payload — see the Persistence section.
- The System Health monitor tallies the wire values `healthy`/`warning`/
  `unhealthy`/`unknown` (displaying `unhealthy` as "critical") — never a
  literal `'critical'` string, which doesn't appear on the wire. See
  [Health](./health.md) for the history of this exact mismatch bug.

## Related

- [Desktop](./desktop.md), [Health](./health.md), [Security](./security.md),
  [API Contract](../API.md)
