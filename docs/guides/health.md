# Health Monitoring

**Purpose:** System health checks and status overview (memory, disk space,
S.M.A.R.T., network, services, temperatures).

## Backend

- `GET /health` (`handle_health_endpoint`, `src/web/mod.rs`) requires a valid
  session, runs `HealthService::run_all_checks()` (`src/health.rs`), and
  reads CPU/memory from `/proc/stat` and `/proc/meminfo`.
  Returns `{ success, data: { checks[], overall_status, cpu_usage, memory_usage } }`
  — field names are `checks`/`overall_status`, not `status`/`metrics`.
- `GET /dashboard/health` (`handle_health_data`) feeds the Dashboard health card.

Status values on the wire: `healthy` / `warning` / `unhealthy` / `unknown`
(the `HealthStatus` Rust enum, `src/health.rs`) — **not** `critical`.
`critical` is only the *display* label the frontend shows for the wire
value `unhealthy` (see `Health.svelte`'s `getStatusLabel`); a past bug
had the frontend's own status type/switches expect a literal
`'critical'` that never actually appears on the wire, silently falling
back to an untranslated default. Don't reintroduce that mismatch in any
new consumer of this endpoint.

## Frontend

- `frontend/src/routes/Health.svelte` (self-contained — renders its check
  list inline; no dedicated card components).
- **Overall status lives in the body, not the header** (2026-09-14). It used to
  be a badge in the window header; the header was standardized to icon + title
  only, so the reading moved to the `data-testid="health-overall"` strip at the
  top of the page body, which keeps the original `data-testid="health-badge"`
  on its label pill. Its heading is the i18n key `health.overall_status`
  (renamed from the now-unused `health.subtitle`, so the four locale files stay
  at key parity).
- `frontend/src/lib/services/api.ts`: `healthCheck`, `getHealthData`.
- The desktop "System Health" monitor widget (`frontend/src/routes/
  Desktop.svelte`, `.hv-health-monitor`) is a second, independent
  consumer of `healthCheck()` — a read-only, at-a-glance tally of the 8
  checks' statuses, owned by the Widgets app rather
  than this one.

## Invariants

- `/health` requires a session (a past audit found dashboard endpoints open).
- Check names map to `health.*` translation keys; the timestamp uses the active UI
  locale.

## Related

- [Dashboard](./dashboard.md), [Desktop](./desktop.md), [API Contract](../API.md)
